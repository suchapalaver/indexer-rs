// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Main action executor for processing approved allocation actions.

use std::{sync::Arc, time::Duration};

use alloy::{
    network::TransactionBuilder,
    primitives::{Address, Bytes, FixedBytes, U256},
    providers::Provider,
    rpc::types::TransactionRequest,
    signers::{local::PrivateKeySigner, Signer},
};
use bip39::Mnemonic;
use sqlx::PgPool;
use thegraph_core::DeploymentId;
use tokio::{sync::watch, time::interval};
use tracing::{debug, error, info, warn};

use super::{
    contracts::{Controller, HorizonStaking, SubgraphService},
    errors::{is_nonce_error, ExecutorError},
    provider::{ExecutorProvider, ProviderCache},
    transactions::{build_allocate_tx, build_unallocate_tx, parse_amount, GAS_BUFFER_PERCENT},
};
use crate::models::{Action, ActionStatus, ActionType};

/// Maximum number of retries for receipt polling.
const RECEIPT_MAX_RETRIES: u32 = 3;

/// Initial backoff duration for receipt polling (in milliseconds).
const RECEIPT_INITIAL_BACKOFF_MS: u64 = 1000;

/// Timeout for waiting for transaction receipt (in seconds).
const RECEIPT_TIMEOUT_SECS: u64 = 60;

/// Configuration for the action executor.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Interval between execution cycles
    pub execution_interval: Duration,

    /// Address of the SubgraphService contract
    pub subgraph_service_address: Address,

    /// Address of the HorizonStaking contract (for authorization checks)
    pub horizon_staking_address: Address,

    /// Address of the Controller contract (for pause checks)
    pub controller_address: Address,

    /// The indexer's address
    pub indexer_address: Address,

    /// Protocol network identifier (e.g., "eip155:42161")
    pub protocol_network: String,

    /// Chain ID for the network
    pub chain_id: u64,

    /// Operator mnemonic for deriving allocation keys
    pub operator_mnemonic: Option<Mnemonic>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            execution_interval: Duration::from_secs(30),
            subgraph_service_address: Address::ZERO,
            horizon_staking_address: Address::ZERO,
            controller_address: Address::ZERO,
            indexer_address: Address::ZERO,
            protocol_network: String::new(),
            chain_id: 0,
            operator_mnemonic: None,
        }
    }
}

/// Action executor that processes approved allocation actions.
pub struct ActionExecutor {
    /// Database connection pool
    pool: PgPool,

    /// Provider cache for nonce management
    provider_cache: Arc<ProviderCache>,

    /// Private key signer for transactions
    signer: PrivateKeySigner,

    /// Executor configuration
    config: ExecutorConfig,

    /// Watch receiver for current epoch number
    epoch_rx: watch::Receiver<u64>,

    /// Counter for allocation index (used in key derivation)
    allocation_index: std::sync::atomic::AtomicU64,
}

impl ActionExecutor {
    /// Create a new action executor.
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `provider_cache` - Provider cache for blockchain connections
    /// * `signer` - Private key signer for transactions
    /// * `config` - Executor configuration
    /// * `epoch_rx` - Watch receiver for current epoch number
    pub fn new(
        pool: PgPool,
        provider_cache: Arc<ProviderCache>,
        signer: PrivateKeySigner,
        config: ExecutorConfig,
        epoch_rx: watch::Receiver<u64>,
    ) -> Self {
        Self {
            pool,
            provider_cache,
            signer,
            config,
            epoch_rx,
            allocation_index: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Run the executor loop.
    ///
    /// This periodically checks for approved actions and executes them.
    pub async fn run(&self) -> Result<(), ExecutorError> {
        let mut ticker = interval(self.config.execution_interval);

        loop {
            ticker.tick().await;

            if let Err(e) = self.execute_approved_actions().await {
                error!(error = %e, "Error executing approved actions");
            }
        }
    }

    /// Execute all approved actions.
    ///
    /// Actions are executed in priority order:
    /// 1. Unallocate (free up stake)
    /// 2. Reallocate (smaller changes)
    /// 3. Allocate (use freed stake)
    pub async fn execute_approved_actions(&self) -> Result<(), ExecutorError> {
        let actions = Action::get_approved(&self.pool, &self.config.protocol_network).await?;

        if actions.is_empty() {
            debug!("No approved actions to execute");
            return Ok(());
        }

        info!(count = actions.len(), "Executing approved actions");

        // Get provider for authorization check
        let provider = self
            .provider_cache
            .get_provider(self.config.chain_id, self.signer.clone())
            .await?;

        // Verify operator authorization before processing any actions
        self.verify_operator_authorization(&provider).await?;

        // Sort actions by type for optimal execution order
        let mut unallocates = Vec::new();
        let mut reallocates = Vec::new();
        let mut allocates = Vec::new();

        for action in actions {
            match action.action_type {
                ActionType::Unallocate => unallocates.push(action),
                ActionType::Reallocate => reallocates.push(action),
                ActionType::Allocate => allocates.push(action),
            }
        }

        // Execute in order: unallocate -> reallocate -> allocate
        for action in unallocates.into_iter().chain(reallocates).chain(allocates) {
            if let Err(e) = self.execute_action(&action).await {
                error!(
                    action_id = action.id,
                    action_type = ?action.action_type,
                    error = %e,
                    "Failed to execute action"
                );

                // Mark action as failed
                let _ = Action::update_status(
                    &self.pool,
                    action.id,
                    &self.config.protocol_network,
                    ActionStatus::Failed,
                    None,
                    Some(&e.to_string()),
                )
                .await;

                // Check for nonce error and reset provider if needed
                if is_nonce_error(&e.to_string()) {
                    self.provider_cache
                        .reset_provider(self.config.chain_id, self.signer.address())
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Execute a single action.
    async fn execute_action(&self, action: &Action) -> Result<(), ExecutorError> {
        info!(
            action_id = action.id,
            action_type = ?action.action_type,
            deployment = %action.deployment_id,
            "Executing action"
        );

        // Mark action as pending
        Action::update_status(
            &self.pool,
            action.id,
            &self.config.protocol_network,
            ActionStatus::Pending,
            None,
            None,
        )
        .await?;

        // Get provider
        let provider = self
            .provider_cache
            .get_provider(self.config.chain_id, self.signer.clone())
            .await?;

        // Build and send transaction based on action type
        let tx_hash = match action.action_type {
            ActionType::Allocate => self.execute_allocate(action, &provider).await?,
            ActionType::Unallocate => self.execute_unallocate(action, &provider).await?,
            ActionType::Reallocate => self.execute_reallocate(action, &provider).await?,
        };

        // Mark action as deploying
        Action::update_status(
            &self.pool,
            action.id,
            &self.config.protocol_network,
            ActionStatus::Deploying,
            Some(&tx_hash),
            None,
        )
        .await?;

        // Wait for receipt with retry
        let receipt = self
            .get_receipt_with_retry(&provider, tx_hash.clone())
            .await?;

        // Check if transaction succeeded
        if !receipt.status() {
            return Err(ExecutorError::TransactionReverted { tx_hash });
        }

        // Mark action as success
        Action::update_status(
            &self.pool,
            action.id,
            &self.config.protocol_network,
            ActionStatus::Success,
            Some(&tx_hash),
            None,
        )
        .await?;

        info!(
            action_id = action.id,
            tx_hash = %tx_hash,
            "Action executed successfully"
        );

        Ok(())
    }

    /// Execute an allocate action.
    async fn execute_allocate(
        &self,
        action: &Action,
        provider: &ExecutorProvider,
    ) -> Result<String, ExecutorError> {
        // Parse amount
        let amount_str = action
            .amount
            .as_ref()
            .ok_or_else(|| ExecutorError::MissingField {
                action_id: action.id,
                field: "amount".to_string(),
            })?;
        let tokens = parse_amount(amount_str).map_err(|mut e| {
            if let ExecutorError::InvalidAmount { action_id, .. } = &mut e {
                *action_id = action.id;
            }
            e
        })?;

        // Parse deployment ID (convert from IPFS hash to bytes32)
        let deployment_id = parse_deployment_id(&action.deployment_id)?;

        // Generate allocation ID and proof
        let (allocation_id, proof) = self
            .generate_allocation_id_and_proof(&deployment_id)
            .await?;

        // Build transaction
        let calldata = build_allocate_tx(
            self.config.indexer_address,
            deployment_id,
            tokens,
            allocation_id,
            proof,
        );

        // Send transaction
        self.send_transaction(provider, calldata).await
    }

    /// Execute an unallocate action.
    async fn execute_unallocate(
        &self,
        action: &Action,
        provider: &ExecutorProvider,
    ) -> Result<String, ExecutorError> {
        // Parse allocation ID
        let allocation_id_str =
            action
                .allocation_id
                .as_ref()
                .ok_or_else(|| ExecutorError::MissingField {
                    action_id: action.id,
                    field: "allocation_id".to_string(),
                })?;
        let allocation_id: Address = allocation_id_str
            .parse()
            .map_err(|e| ExecutorError::TransactionBuild(format!("invalid allocation ID: {e}")))?;

        // Parse POI
        let poi = if let Some(poi_str) = &action.poi {
            poi_str
                .parse()
                .map_err(|e| ExecutorError::TransactionBuild(format!("invalid POI: {e}")))?
        } else {
            FixedBytes::ZERO // Force close with zero POI
        };

        let poi_block_number = action.poi_block_number.unwrap_or(0) as u64;

        // Check if over-allocated
        let contract = SubgraphService::new(self.config.subgraph_service_address, provider);
        let is_over_allocated = contract
            .isOverAllocated(self.config.indexer_address)
            .call()
            .await
            .unwrap_or(false);

        // Build transaction
        let calldata = build_unallocate_tx(
            self.config.indexer_address,
            allocation_id,
            poi,
            poi_block_number,
            None,
            is_over_allocated,
        );

        // Send transaction
        self.send_transaction(provider, calldata).await
    }

    /// Execute a reallocate action.
    async fn execute_reallocate(
        &self,
        action: &Action,
        provider: &ExecutorProvider,
    ) -> Result<String, ExecutorError> {
        // For reallocate, we execute unallocate first, then allocate
        // This could be optimized with batching in the future

        // Execute unallocate
        let _unallocate_hash = self.execute_unallocate(action, provider).await?;

        // Execute allocate with new amount
        self.execute_allocate(action, provider).await
    }

    /// Send a transaction to the SubgraphService contract.
    async fn send_transaction(
        &self,
        provider: &ExecutorProvider,
        calldata: Bytes,
    ) -> Result<String, ExecutorError> {
        // Check if network is paused before submitting
        self.check_network_paused(provider).await?;

        let signer_address = self.signer.address();

        // Build transaction request
        let tx = TransactionRequest::default()
            .with_to(self.config.subgraph_service_address)
            .with_input(calldata);

        // Estimate gas
        let gas_estimate = provider
            .estimate_gas(tx.clone())
            .await
            .map_err(|e| ExecutorError::GasEstimation(e.to_string()))?;

        // Add buffer
        let gas_limit = gas_estimate.saturating_mul(GAS_BUFFER_PERCENT) / 100;

        // Check balance
        let fee_estimate = provider
            .get_gas_price()
            .await
            .map_err(|e| ExecutorError::Provider(format!("failed to get gas price: {e}")))?;
        let estimated_cost = U256::from(gas_limit) * U256::from(fee_estimate);
        ProviderCache::check_gas_balance(provider, signer_address, estimated_cost).await?;

        // Build final transaction with gas limit
        let tx = tx.with_gas_limit(gas_limit);

        // Send transaction
        let pending_tx = provider
            .send_transaction(tx)
            .await
            .map_err(|e| ExecutorError::TransactionSubmit(e.to_string()))?;

        let tx_hash = format!("{:?}", pending_tx.tx_hash());
        info!(tx_hash = %tx_hash, gas_limit = gas_limit, "Transaction submitted");

        Ok(tx_hash)
    }

    /// Get transaction receipt with retry logic.
    async fn get_receipt_with_retry(
        &self,
        provider: &ExecutorProvider,
        tx_hash: String,
    ) -> Result<alloy::rpc::types::TransactionReceipt, ExecutorError> {
        let tx_hash_bytes: FixedBytes<32> = tx_hash
            .parse()
            .map_err(|e| ExecutorError::Provider(format!("invalid tx hash: {e}")))?;

        // First attempt: wait with timeout
        match tokio::time::timeout(
            Duration::from_secs(RECEIPT_TIMEOUT_SECS),
            provider.get_transaction_receipt(tx_hash_bytes),
        )
        .await
        {
            Ok(Ok(Some(receipt))) => return Ok(receipt),
            Ok(Ok(None)) => {
                debug!(tx_hash = %tx_hash, "Receipt not found on first attempt");
            }
            Ok(Err(e)) => {
                warn!(tx_hash = %tx_hash, error = %e, "Error getting receipt");
            }
            Err(_) => {
                warn!(tx_hash = %tx_hash, "Timeout waiting for receipt");
            }
        }

        // Retry with exponential backoff
        let mut backoff = RECEIPT_INITIAL_BACKOFF_MS;
        for attempt in 1..=RECEIPT_MAX_RETRIES {
            tokio::time::sleep(Duration::from_millis(backoff)).await;

            match provider.get_transaction_receipt(tx_hash_bytes).await {
                Ok(Some(receipt)) => {
                    info!(tx_hash = %tx_hash, attempt = attempt, "Receipt found on retry");
                    return Ok(receipt);
                }
                Ok(None) => {
                    debug!(tx_hash = %tx_hash, attempt = attempt, "Receipt still not found");
                }
                Err(e) => {
                    warn!(tx_hash = %tx_hash, attempt = attempt, error = %e, "Error on retry");
                }
            }

            backoff *= 2; // Exponential backoff
        }

        // Final check: verify transaction exists
        match provider.get_transaction_by_hash(tx_hash_bytes).await {
            Ok(Some(_)) => Err(ExecutorError::TransactionPending { tx_hash }),
            Ok(None) => Err(ExecutorError::TransactionNotFound {
                tx_hash,
                retries: RECEIPT_MAX_RETRIES,
            }),
            Err(e) => Err(ExecutorError::Provider(format!(
                "failed to verify transaction: {e}"
            ))),
        }
    }

    /// Generate an allocation ID and its corresponding proof.
    ///
    /// The allocation ID is the address of a wallet derived from the operator mnemonic,
    /// the current epoch, the deployment ID, and an incrementing index.
    ///
    /// The proof is an EIP-712 signature over the allocation ID, signed by the derived
    /// wallet. This proves that the indexer controls the allocation ID address.
    async fn generate_allocation_id_and_proof(
        &self,
        deployment_id: &FixedBytes<32>,
    ) -> Result<(Address, Bytes), ExecutorError> {
        // Get the operator mnemonic
        let mnemonic =
            self.config
                .operator_mnemonic
                .as_ref()
                .ok_or_else(|| ExecutorError::MissingField {
                    action_id: 0,
                    field: "operator_mnemonic".to_string(),
                })?;

        // Get current epoch
        let current_epoch = *self.epoch_rx.borrow();

        // Get next allocation index and increment atomically
        let index = self
            .allocation_index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Convert deployment ID to DeploymentId type for key derivation
        let deployment = DeploymentId::new(*deployment_id);

        // Derive the allocation key pair using the attestation crate's derivation logic
        let allocation_wallet = indexer_attestation::derive_key_pair(
            mnemonic.to_string().as_str(),
            current_epoch,
            &deployment,
            index,
        )
        .map_err(|e| {
            ExecutorError::TransactionBuild(format!("failed to derive allocation key: {e}"))
        })?;

        let allocation_id = allocation_wallet.address();

        // Sign the allocation ID to create the proof
        // The proof demonstrates that the operator controls the allocation ID
        let proof = self
            .sign_allocation_proof(&allocation_wallet, allocation_id)
            .await?;

        info!(
            allocation_id = %allocation_id,
            epoch = current_epoch,
            index = index,
            deployment = %deployment,
            "Generated allocation ID and proof"
        );

        Ok((allocation_id, proof))
    }

    /// Sign an allocation proof using the allocation wallet.
    ///
    /// This creates a signature that proves the indexer controls the allocation ID address.
    /// The signature is over a message containing the allocation ID.
    async fn sign_allocation_proof(
        &self,
        allocation_wallet: &PrivateKeySigner,
        allocation_id: Address,
    ) -> Result<Bytes, ExecutorError> {
        // Create the message to sign: the allocation ID as bytes
        // This follows the Graph Protocol pattern where the proof is a signature
        // from the allocation wallet over its own address
        let message = allocation_id.as_slice();

        // Sign the message using EIP-191 personal_sign
        let signature = allocation_wallet.sign_message(message).await.map_err(|e| {
            ExecutorError::TransactionBuild(format!("failed to sign allocation proof: {e}"))
        })?;

        // Convert signature to bytes
        Ok(Bytes::from(signature.as_bytes().to_vec()))
    }

    /// Verify that the operator is authorized to act on behalf of the indexer.
    ///
    /// This checks the HorizonStaking contract to ensure the operator (signer) has
    /// been granted authorization by the indexer (service provider) for the
    /// SubgraphService (verifier).
    ///
    /// This check is performed once per execution cycle, not per-action, to minimize
    /// RPC calls while still ensuring authorization before any transactions are submitted.
    async fn verify_operator_authorization(
        &self,
        provider: &ExecutorProvider,
    ) -> Result<(), ExecutorError> {
        let staking = HorizonStaking::new(self.config.horizon_staking_address, provider);

        let operator_address = self.signer.address();

        // Check if the operator is authorized for the indexer on the SubgraphService
        let is_authorized = staking
            .isAuthorized(
                self.config.indexer_address,
                operator_address,
                self.config.subgraph_service_address,
            )
            .call()
            .await
            .map_err(|e| ExecutorError::ContractCall(format!("isAuthorized check failed: {e}")))?;

        if !is_authorized {
            warn!(
                indexer = %self.config.indexer_address,
                operator = %operator_address,
                verifier = %self.config.subgraph_service_address,
                "Operator is not authorized for indexer"
            );
            return Err(ExecutorError::UnauthorizedOperator {
                indexer: self.config.indexer_address,
                operator: operator_address,
            });
        }

        debug!(
            indexer = %self.config.indexer_address,
            operator = %operator_address,
            "Operator authorization verified"
        );

        Ok(())
    }

    /// Check if the network is paused.
    ///
    /// Queries the Controller contract to determine if the protocol is currently paused.
    /// When paused, no state-changing transactions should be submitted as they will fail.
    ///
    /// This check is performed before each transaction submission to ensure we don't
    /// waste gas on transactions that will definitely fail.
    async fn check_network_paused(&self, provider: &ExecutorProvider) -> Result<(), ExecutorError> {
        let controller = Controller::new(self.config.controller_address, provider);

        let paused = controller
            .paused()
            .call()
            .await
            .map_err(|e| ExecutorError::ContractCall(format!("paused check failed: {e}")))?;

        if paused {
            warn!("Network is paused, cannot submit transactions");
            return Err(ExecutorError::NetworkPaused);
        }

        debug!("Network pause check passed");
        Ok(())
    }
}

/// Parse a deployment ID from IPFS hash or bytes32 format.
///
/// Supports both IPFS CIDv0 hashes (starting with "Qm") and hex-encoded bytes32.
fn parse_deployment_id(deployment_id: &str) -> Result<FixedBytes<32>, ExecutorError> {
    // Use thegraph_core::DeploymentId which handles both IPFS and hex formats
    let parsed: DeploymentId = deployment_id
        .parse()
        .map_err(|e| ExecutorError::TransactionBuild(format!("invalid deployment ID: {e}")))?;

    // Convert to bytes32 using the explicit AsRef<[u8; 32]> implementation
    let bytes: &[u8; 32] = parsed.as_ref();
    Ok(FixedBytes::from(*bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.execution_interval, Duration::from_secs(30));
        assert_eq!(config.subgraph_service_address, Address::ZERO);
        assert_eq!(config.horizon_staking_address, Address::ZERO);
        assert_eq!(config.controller_address, Address::ZERO);
        assert_eq!(config.indexer_address, Address::ZERO);
    }

    #[test]
    fn test_parse_deployment_id_hex() {
        // Should work for bytes32 hex
        let hex = "0x0000000000000000000000000000000000000000000000000000000000000000";
        let result = parse_deployment_id(hex);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), FixedBytes::ZERO);
    }

    #[test]
    fn test_parse_deployment_id_ipfs() {
        // Should work for valid IPFS CIDv0 hashes
        let ipfs_hash = "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY";
        let result = parse_deployment_id(ipfs_hash);
        assert!(result.is_ok());

        // The bytes32 representation should be non-zero
        let bytes = result.unwrap();
        assert_ne!(bytes, FixedBytes::ZERO);

        // Verify round-trip: same IPFS hash should produce same bytes32
        let result2 = parse_deployment_id(ipfs_hash);
        assert_eq!(bytes, result2.unwrap());
    }

    #[test]
    fn test_parse_deployment_id_invalid() {
        // Should fail for invalid strings
        let result = parse_deployment_id("not-a-valid-id");
        assert!(result.is_err());

        // Should fail for too short hex
        let result = parse_deployment_id("0x1234");
        assert!(result.is_err());
    }
}
