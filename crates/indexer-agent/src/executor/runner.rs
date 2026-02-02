// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Main action executor for processing approved allocation actions.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy::{
    network::TransactionBuilder,
    primitives::{Address, BlockNumber, Bytes, FixedBytes, B256, U256},
    providers::Provider,
    rpc::types::TransactionRequest,
    signers::{local::PrivateKeySigner, Signer},
    sol_types::SolStruct,
};
use bip39::Mnemonic;
use sqlx::PgPool;
use thegraph_core::{DeploymentId, IndexerId, ProofOfIndexing};
use tokio::{sync::watch, time::interval};
use tracing::{debug, error, info, warn};
use url::Url;

use super::{
    contracts::{
        subgraph_service_eip712_domain, AllocationIdProof, Controller, HorizonStaking,
        SubgraphService,
    },
    errors::{is_nonce_error, ExecutorError},
    provider::{ExecutorProvider, ProviderCache},
    transactions::{build_allocate_tx, build_unallocate_tx, parse_amount, GAS_BUFFER_PERCENT},
};
use crate::{
    metrics,
    models::{Action, ActionStatus, ActionType},
    poi::PoiResolver,
};

/// Maximum number of retries for receipt polling.
const RECEIPT_MAX_RETRIES: u32 = 3;

/// Initial backoff duration for receipt polling (in milliseconds).
const RECEIPT_INITIAL_BACKOFF_MS: u64 = 1000;

/// Timeout for waiting for transaction receipt (in seconds).
const RECEIPT_TIMEOUT_SECS: u64 = 60;

/// Default timeout for waiting on gas price (5 minutes).
const DEFAULT_GAS_PRICE_WAIT_TIMEOUT_SECS: u64 = 300;

/// Poll interval when waiting for gas price to drop.
const GAS_PRICE_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Maximum allocation index to try when generating allocation IDs.
/// Matches TypeScript behavior of checking [0..100).
const MAX_ALLOCATION_INDEX: u64 = 100;

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
    pub indexer_id: IndexerId,

    /// Protocol network identifier (e.g., "eip155:42161")
    pub protocol_network: String,

    /// Chain ID for the network
    pub chain_id: u64,

    /// Operator mnemonic for deriving allocation keys
    pub operator_mnemonic: Option<Mnemonic>,

    /// Maximum gas price (in gwei) to accept before waiting.
    /// If None, no gas price limit is enforced.
    pub max_gas_price_gwei: Option<u64>,

    /// Maximum time (seconds) to wait for acceptable gas price.
    pub gas_price_wait_timeout_secs: u64,

    /// Graph-node status URL for POI resolution.
    /// Required for closing allocations with proper POI.
    pub graph_node_status_url: Option<Url>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            execution_interval: Duration::from_secs(30),
            subgraph_service_address: Address::ZERO,
            horizon_staking_address: Address::ZERO,
            controller_address: Address::ZERO,
            indexer_id: IndexerId::new(Address::ZERO),
            protocol_network: String::new(),
            chain_id: 0,
            operator_mnemonic: None,
            max_gas_price_gwei: None,
            gas_price_wait_timeout_secs: DEFAULT_GAS_PRICE_WAIT_TIMEOUT_SECS,
            graph_node_status_url: None,
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

    /// POI resolver for querying graph-node
    poi_resolver: Option<PoiResolver>,
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
        // Create POI resolver if graph-node status URL is configured
        let poi_resolver = config
            .graph_node_status_url
            .as_ref()
            .map(|url| PoiResolver::new(url.clone()));

        if poi_resolver.is_none() {
            warn!(
                "Graph-node status URL not configured. \
                POI resolution will be unavailable - all unallocate actions will require force=true"
            );
        }

        Self {
            pool,
            provider_cache,
            signer,
            config,
            epoch_rx,
            poi_resolver,
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
            let action_type_str = action_type_to_str(&action.action_type);
            if let Err(e) = self.execute_action(&action).await {
                error!(
                    action_id = action.id,
                    action_type = ?action.action_type,
                    error = %e,
                    "Failed to execute action"
                );

                // Record transaction failure metric
                metrics::record_transaction_failed(action_type_str);

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

        let action_type_str = action_type_to_str(&action.action_type);
        let gas_used = receipt.gas_used;

        // Check if transaction succeeded
        if !receipt.status() {
            metrics::record_transaction_reverted(action_type_str, gas_used);
            return Err(ExecutorError::TransactionReverted { tx_hash });
        }

        // Record successful transaction
        metrics::record_transaction_success(action_type_str, gas_used);

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
            gas_used = gas_used,
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

        // Generate allocation ID and proof, checking against existing on-chain allocations
        let (allocation_id, proof) = self
            .generate_allocation_id_and_proof(&deployment_id, provider)
            .await?;

        // Build transaction
        let calldata = build_allocate_tx(
            self.config.indexer_id.into_inner(),
            deployment_id,
            tokens,
            allocation_id,
            proof,
        );

        // Send transaction
        self.send_transaction(provider, calldata).await
    }

    /// Execute an unallocate action.
    ///
    /// POI Resolution Behavior:
    /// - If `action.poi` is set, use that POI directly (Management API or manual override)
    /// - If `action.force` is true, use zero POI (forfeits indexing rewards)
    /// - Otherwise, resolve POI from graph-node at the deployment's latest synced block
    /// - If POI resolution fails and force=false, the action fails with an error
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

        // Parse deployment ID for verification and POI resolution
        let deployment_id_bytes = parse_deployment_id(&action.deployment_id)?;
        let deployment_id: DeploymentId = action
            .deployment_id
            .parse()
            .map_err(|e| ExecutorError::TransactionBuild(format!("invalid deployment ID: {e}")))?;

        // Verify allocation exists and is valid before building transaction
        self.verify_allocation_exists(provider, allocation_id, Some(&deployment_id_bytes))
            .await?;

        // Resolve POI based on action parameters
        let (poi, poi_block_number, public_poi) =
            self.resolve_poi_for_action(action, &deployment_id).await?;

        info!(
            action_id = action.id,
            allocation_id = %allocation_id,
            deployment = %action.deployment_id,
            poi = %poi,
            poi_block_number = poi_block_number,
            has_public_poi = public_poi.is_some(),
            force = action.force.unwrap_or(false),
            "Resolved POI for unallocate action"
        );

        // Check if over-allocated
        let contract = SubgraphService::new(self.config.subgraph_service_address, provider);
        let is_over_allocated = contract
            .isOverAllocated(self.config.indexer_id.into_inner())
            .call()
            .await
            .map_err(|e| {
                ExecutorError::ContractCall(format!("Failed to check over-allocation status: {e}"))
            })?;

        // Build transaction
        let calldata = build_unallocate_tx(
            self.config.indexer_id.into_inner(),
            allocation_id,
            poi,
            poi_block_number,
            public_poi,
            is_over_allocated,
        );

        // Send transaction
        self.send_transaction(provider, calldata).await
    }

    /// Resolve POI for an unallocate or reallocate action.
    ///
    /// This method implements the POI resolution logic:
    /// 1. If `action.poi` is set, use that POI directly (explicit override)
    /// 2. If `action.force` is true, return zero POI (force close)
    /// 3. Otherwise, query graph-node for POI at the latest synced block
    ///
    /// # Returns
    /// A tuple of (poi, block_number, public_poi) where:
    /// - `poi` is the POI hash to submit (bytes32)
    /// - `block_number` is the block number for the POI
    /// - `public_poi` is the optional public POI for metadata
    async fn resolve_poi_for_action(
        &self,
        action: &Action,
        deployment: &DeploymentId,
    ) -> Result<(ProofOfIndexing, BlockNumber, Option<ProofOfIndexing>), ExecutorError> {
        // Case 1: Explicit POI provided in action (from Management API or manual queue)
        if let Some(poi_str) = &action.poi {
            let poi: ProofOfIndexing = poi_str
                .parse::<B256>()
                .map(ProofOfIndexing::from)
                .map_err(|e| ExecutorError::TransactionBuild(format!("invalid POI: {e}")))?;

            let Some(block_number_raw) = action.poi_block_number else {
                return Err(ExecutorError::TransactionBuild(
                    "missing POI block number".into(),
                ));
            };
            if block_number_raw <= 0 {
                return Err(ExecutorError::TransactionBuild(
                    "invalid POI block number".into(),
                ));
            }
            let block_number: BlockNumber = block_number_raw
                .try_into()
                .map_err(|_| ExecutorError::TransactionBuild("invalid POI block number".into()))?;

            // Parse public_poi if provided
            let public_poi = if let Some(public_poi_str) = &action.public_poi {
                Some(
                    public_poi_str
                        .parse::<B256>()
                        .map(ProofOfIndexing::from)
                        .map_err(|e| {
                            ExecutorError::TransactionBuild(format!("invalid public POI: {e}"))
                        })?,
                )
            } else {
                None
            };

            debug!(
                action_id = action.id,
                poi = %poi,
                block_number = block_number,
                "Using explicit POI from action"
            );

            return Ok((poi, block_number, public_poi));
        }

        // Case 2: Force close - use zero POI (forfeits indexing rewards)
        if action.force == Some(true) {
            warn!(
                action_id = action.id,
                deployment = %deployment,
                "Force closing allocation with zero POI - indexing rewards will be forfeited"
            );
            return Ok((ProofOfIndexing::ZERO, BlockNumber::from(0u64), None));
        }

        // Case 3: Resolve POI from graph-node
        let Some(ref poi_resolver) = self.poi_resolver else {
            return Err(ExecutorError::PoiRequired {
                action_id: action.id,
            });
        };

        debug!(
            action_id = action.id,
            deployment = %deployment,
            "Resolving POI from graph-node"
        );

        let poi_result = poi_resolver
            .resolve_poi_at_latest(deployment)
            .await
            .map_err(|e| ExecutorError::PoiResolutionFailed {
                deployment: deployment.to_string(),
                reason: e.to_string(),
            })?;

        // For public POI resolution, we use the public POI as both the POI and public_poi
        // This is because graph-node returns the public POI, which is what we submit
        Ok((
            poi_result.public_poi,
            poi_result.block_number,
            Some(poi_result.public_poi),
        ))
    }

    /// Execute a reallocate action.
    ///
    /// # WARNING: Non-Atomic Operation
    ///
    /// This executes two separate transactions:
    /// 1. Unallocate (close existing allocation)
    /// 2. Allocate (create new allocation)
    ///
    /// If the first succeeds but the second fails, the original allocation
    /// is closed and stake returns to the indexer's pool. Operators must
    /// re-allocate manually or retry. A future improvement is to use
    /// SubgraphService multicall for atomicity.
    async fn execute_reallocate(
        &self,
        action: &Action,
        provider: &ExecutorProvider,
    ) -> Result<String, ExecutorError> {
        // For reallocate, we execute unallocate first, then allocate
        // This could be optimized with batching in the future

        // Execute unallocate
        let unallocate_hash = self.execute_unallocate(action, provider).await?;

        // Record unallocate tx hash for auditability
        Action::set_unallocate_transaction(
            &self.pool,
            action.id,
            &self.config.protocol_network,
            &unallocate_hash,
        )
        .await?;

        // Execute allocate with new amount
        match self.execute_allocate(action, provider).await {
            Ok(tx_hash) => Ok(tx_hash),
            Err(e) => {
                warn!(
                    action_id = action.id,
                    deployment = %action.deployment_id,
                    error = %e,
                    unallocate_tx = %unallocate_hash,
                    "Reallocate partial failure: unallocate succeeded but allocate failed"
                );
                metrics::record_reallocate_partial_failure();
                Err(e)
            }
        }
    }

    /// Send a transaction to the SubgraphService contract.
    async fn send_transaction(
        &self,
        provider: &ExecutorProvider,
        calldata: Bytes,
    ) -> Result<String, ExecutorError> {
        // Check if network is paused before submitting
        self.check_network_paused(provider).await?;

        // Wait for acceptable gas price if threshold is configured
        self.wait_for_acceptable_gas_price(provider).await?;

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

    /// Wait for gas price to drop below the configured threshold.
    ///
    /// If no gas price threshold is configured (`max_gas_price_gwei = None`),
    /// this method returns immediately without any checks.
    ///
    /// If a threshold is configured, this method polls the gas price every 15 seconds
    /// until either:
    /// - The gas price drops below the threshold, or
    /// - The timeout expires (default 5 minutes)
    ///
    /// This implements Invariant 25.2: Gas Price Threshold from the TypeScript agent.
    async fn wait_for_acceptable_gas_price(
        &self,
        provider: &ExecutorProvider,
    ) -> Result<(), ExecutorError> {
        let Some(max_gwei) = self.config.max_gas_price_gwei else {
            // No threshold configured, proceed immediately
            metrics::record_gas_price_immediate();
            return Ok(());
        };

        let timeout_secs = self.config.gas_price_wait_timeout_secs;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
        let start = Instant::now();

        // Convert threshold to wei (1 gwei = 10^9 wei)
        let max_wei: u128 = (max_gwei as u128) * 1_000_000_000;

        let mut first_check = true;
        loop {
            let current_gas_price = provider
                .get_gas_price()
                .await
                .map_err(|e| ExecutorError::Provider(format!("failed to get gas price: {e}")))?;

            let current_wei: u128 = current_gas_price;
            let current_gwei = current_wei / 1_000_000_000;

            if current_wei <= max_wei {
                debug!(
                    current_gwei = current_gwei,
                    max_gwei = max_gwei,
                    "Gas price acceptable"
                );

                if first_check {
                    // Gas was acceptable on first check
                    metrics::record_gas_price_immediate();
                } else {
                    // Had to wait for gas price to drop
                    let wait_secs = start.elapsed().as_secs_f64();
                    metrics::record_gas_price_waited(wait_secs);
                }
                return Ok(());
            }

            if tokio::time::Instant::now() >= deadline {
                let wait_secs = start.elapsed().as_secs_f64();
                warn!(
                    current_gwei = current_gwei,
                    max_gwei = max_gwei,
                    waited_secs = timeout_secs,
                    "Gas price threshold timeout"
                );
                metrics::record_gas_price_timeout(wait_secs);
                return Err(ExecutorError::GasPriceThresholdTimeout {
                    current_gwei,
                    max_gwei,
                    waited_secs: timeout_secs,
                });
            }

            first_check = false;
            info!(
                current_gwei = current_gwei,
                max_gwei = max_gwei,
                "Gas price above threshold, waiting"
            );

            tokio::time::sleep(GAS_PRICE_POLL_INTERVAL).await;
        }
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
    /// the current epoch, the deployment ID, and an index. This method tries indices
    /// from 0 to 99, checking each derived allocation ID against the on-chain contract
    /// to find one that is not already in use.
    ///
    /// The proof is an EIP-712 signature over the allocation ID, signed by the derived
    /// wallet. This proves that the indexer controls the allocation ID address.
    async fn generate_allocation_id_and_proof(
        &self,
        deployment_id: &FixedBytes<32>,
        provider: &ExecutorProvider,
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

        // Convert deployment ID to DeploymentId type for key derivation
        let deployment = DeploymentId::new(*deployment_id);

        // Create contract instance for checking existing allocations
        let subgraph_service = SubgraphService::new(self.config.subgraph_service_address, provider);

        // Try indices [0..MAX_ALLOCATION_INDEX) until we find an unused allocation ID
        for index in 0..MAX_ALLOCATION_INDEX {
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
            let allocation_id_str = allocation_id.to_string();

            // Skip IDs already present in the local action queue to avoid collisions on restart.
            if Action::allocation_id_exists(
                &self.pool,
                &self.config.protocol_network,
                &allocation_id_str,
            )
            .await?
            {
                debug!(
                    allocation_id = %allocation_id,
                    index = index,
                    "Allocation ID already present in action queue, trying next index"
                );
                continue;
            }

            // Check if this allocation ID is already in use on-chain
            let allocation = subgraph_service
                .getAllocation(allocation_id)
                .call()
                .await
                .map_err(|e| {
                    ExecutorError::ContractCall(format!("getAllocation check failed: {e}"))
                })?;

            // If indexer is zero address, the allocation doesn't exist - we can use this ID
            if allocation.indexer == Address::ZERO {
                // Sign the allocation ID to create the proof
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

                return Ok((allocation_id, proof));
            }

            debug!(
                allocation_id = %allocation_id,
                index = index,
                existing_indexer = %allocation.indexer,
                "Allocation ID already in use, trying next index"
            );
        }

        // All indices exhausted
        warn!(
            deployment = %deployment,
            epoch = current_epoch,
            max_index = MAX_ALLOCATION_INDEX,
            "All allocation ID indices exhausted"
        );

        Err(ExecutorError::AllocationIdExhausted {
            deployment: deployment.to_string(),
            epoch: current_epoch,
            max_index: MAX_ALLOCATION_INDEX,
        })
    }

    /// Sign an allocation proof using the allocation wallet.
    ///
    /// This creates an EIP-712 typed data signature that proves the operator controls
    /// the allocation ID address. The Horizon SubgraphService contract verifies this
    /// signature using the EIP-712 domain:
    /// - name: "SubgraphService"
    /// - version: "1.0"
    /// - chainId: the network's chain ID
    /// - verifyingContract: the SubgraphService contract address
    ///
    /// The typed data structure is:
    /// - AllocationIdProof(address indexer, address allocationId)
    ///
    /// The signature must be from the allocation wallet (whose address is the allocation ID).
    /// The contract recovers the signer and verifies it equals the allocation ID.
    async fn sign_allocation_proof(
        &self,
        allocation_wallet: &PrivateKeySigner,
        allocation_id: Address,
    ) -> Result<Bytes, ExecutorError> {
        // Create the EIP-712 domain for SubgraphService
        let domain = subgraph_service_eip712_domain(
            self.config.chain_id,
            self.config.subgraph_service_address,
        );

        // Create the allocation proof struct
        let proof_data = AllocationIdProof {
            indexer: self.config.indexer_id.into_inner(),
            allocationId: allocation_id,
        };

        // Compute the EIP-712 signing hash
        let signing_hash = proof_data.eip712_signing_hash(&domain);

        // Sign the hash using the allocation wallet
        let signature = allocation_wallet
            .sign_hash(&signing_hash)
            .await
            .map_err(|e| {
                ExecutorError::TransactionBuild(format!("failed to sign allocation proof: {e}"))
            })?;

        // Convert signature to bytes (65 bytes: r, s, v)
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
                self.config.indexer_id.into_inner(),
                operator_address,
                self.config.subgraph_service_address,
            )
            .call()
            .await
            .map_err(|e| ExecutorError::ContractCall(format!("isAuthorized check failed: {e}")))?;

        if !is_authorized {
            warn!(
                indexer = %self.config.indexer_id,
                operator = %operator_address,
                verifier = %self.config.subgraph_service_address,
                "Operator is not authorized for indexer"
            );
            return Err(ExecutorError::UnauthorizedOperator {
                indexer: self.config.indexer_id.into_inner(),
                operator: operator_address,
            });
        }

        debug!(
            indexer = %self.config.indexer_id,
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

    /// Verify that an allocation exists and is in a valid state for unallocation.
    ///
    /// This check ensures that:
    /// 1. The allocation exists on-chain (indexer is not zero address)
    /// 2. The allocation is active (closedAt == 0)
    /// 3. The allocation belongs to the expected indexer
    /// 4. The allocation is for the expected deployment (if provided)
    ///
    /// This prevents wasted gas on transactions that would definitely revert, and
    /// provides clearer error messages than contract reverts.
    async fn verify_allocation_exists(
        &self,
        provider: &ExecutorProvider,
        allocation_id: Address,
        expected_deployment: Option<&FixedBytes<32>>,
    ) -> Result<(), ExecutorError> {
        let subgraph_service = SubgraphService::new(self.config.subgraph_service_address, provider);

        let allocation = subgraph_service
            .getAllocation(allocation_id)
            .call()
            .await
            .map_err(|e| ExecutorError::ContractCall(format!("getAllocation failed: {e}")))?;

        // Check if allocation exists (zero indexer means not found)
        if allocation.indexer == Address::ZERO {
            warn!(
                allocation_id = %allocation_id,
                "Allocation not found on-chain"
            );
            return Err(ExecutorError::AllocationNotFound { allocation_id });
        }

        // Check if allocation is active (closedAt == 0 means active)
        if !allocation.closedAt.is_zero() {
            let closed_at: u64 = allocation.closedAt.try_into().unwrap_or(0);
            warn!(
                allocation_id = %allocation_id,
                closed_at = closed_at,
                "Allocation is not active (already closed)"
            );
            return Err(ExecutorError::AllocationNotActive {
                allocation_id,
                closed_at,
            });
        }

        // Check indexer matches
        if allocation.indexer != self.config.indexer_id.into_inner() {
            warn!(
                allocation_id = %allocation_id,
                expected_indexer = %self.config.indexer_id,
                actual_indexer = %allocation.indexer,
                "Allocation belongs to different indexer"
            );
            return Err(ExecutorError::AllocationIndexerMismatch {
                allocation_id,
                expected_indexer: self.config.indexer_id.into_inner(),
                actual_indexer: allocation.indexer,
            });
        }

        // Check deployment matches (if expected deployment provided)
        if let Some(expected) = expected_deployment {
            if allocation.subgraphDeploymentId != *expected {
                warn!(
                    allocation_id = %allocation_id,
                    expected_deployment = %expected,
                    actual_deployment = %allocation.subgraphDeploymentId,
                    "Allocation is for different deployment"
                );
                return Err(ExecutorError::AllocationDeploymentMismatch {
                    allocation_id,
                    expected_deployment: format!("{expected}"),
                    actual_deployment: format!("{}", allocation.subgraphDeploymentId),
                });
            }
        }

        debug!(
            allocation_id = %allocation_id,
            indexer = %allocation.indexer,
            deployment = %allocation.subgraphDeploymentId,
            "Allocation verification passed"
        );

        Ok(())
    }
}

/// Convert an ActionType to a string for metrics labels.
fn action_type_to_str(action_type: &ActionType) -> &'static str {
    match action_type {
        ActionType::Allocate => "allocate",
        ActionType::Unallocate => "unallocate",
        ActionType::Reallocate => "reallocate",
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
    use sqlx::postgres::PgPoolOptions;
    use thegraph_core::allocation_id;

    use super::*;

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.execution_interval, Duration::from_secs(30));
        assert_eq!(config.subgraph_service_address, Address::ZERO);
        assert_eq!(config.horizon_staking_address, Address::ZERO);
        assert_eq!(config.controller_address, Address::ZERO);
        assert_eq!(config.indexer_id.into_inner(), Address::ZERO);
        assert!(config.max_gas_price_gwei.is_none());
        assert_eq!(config.gas_price_wait_timeout_secs, 300);
        assert!(config.graph_node_status_url.is_none());
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

    #[test]
    fn test_allocation_proof_eip712_structure() {
        use alloy::{primitives::address, sol_types::SolStruct};

        // Test that the AllocationIdProof struct has the correct EIP-712 type hash
        // Expected: keccak256("AllocationIdProof(address indexer,address allocationId)")
        let expected_type_hash =
            alloy::primitives::keccak256("AllocationIdProof(address indexer,address allocationId)");

        // Create a test proof
        let indexer = address!("1234567890123456789012345678901234567890");
        let allocation_id = address!("abcdefabcdefabcdefabcdefabcdefabcdefabcd");

        let proof = AllocationIdProof {
            indexer,
            allocationId: allocation_id,
        };

        // The eip712_type_hash should match expected
        // Note: eip712_type_hash is an instance method from SolStruct trait
        assert_eq!(proof.eip712_type_hash(), expected_type_hash);

        // Create a domain matching SubgraphService contract
        let domain = subgraph_service_eip712_domain(
            42161,                                                // Arbitrum chain ID
            address!("94dc3B65AF05a7A8d36B877eb5DE68B6B16B6389"), // Example SubgraphService address
        );

        // Compute signing hash (this is what gets signed)
        let signing_hash = proof.eip712_signing_hash(&domain);

        // Verify signing hash is non-zero (actual correctness is verified by contract acceptance)
        assert_ne!(signing_hash, alloy::primitives::B256::ZERO);
    }

    #[test]
    fn test_allocation_proof_eip712_manual_hash_matches() {
        use alloy::{
            primitives::{address, keccak256},
            sol_types::{SolStruct, SolValue},
        };

        let indexer = address!("1234567890123456789012345678901234567890");
        let allocation_id = address!("abcdefabcdefabcdefabcdefabcdefabcdefabcd");
        let subgraph_service = address!("94dc3B65AF05a7A8d36B877eb5DE68B6B16B6389");
        let domain = subgraph_service_eip712_domain(42161, subgraph_service);

        let proof = AllocationIdProof {
            indexer,
            allocationId: allocation_id,
        };

        let type_hash = proof.eip712_type_hash();
        let mut encoded = Vec::with_capacity(32 + proof.abi_encoded_size());
        encoded.extend_from_slice(type_hash.as_slice());
        encoded.extend_from_slice(&proof.abi_encode());
        let struct_hash = keccak256(encoded);

        let mut digest = Vec::with_capacity(2 + 32 + 32);
        digest.extend_from_slice(&[0x19, 0x01]);
        digest.extend_from_slice(domain.separator().as_slice());
        digest.extend_from_slice(struct_hash.as_slice());
        let manual = keccak256(digest);

        let auto = proof.eip712_signing_hash(&domain);
        assert_eq!(manual, auto);
    }

    #[tokio::test]
    async fn test_allocation_proof_signature_recovery() {
        use alloy::{primitives::address, signers::local::PrivateKeySigner, sol_types::SolStruct};

        // Create a test allocation wallet
        let allocation_wallet = PrivateKeySigner::random();
        let allocation_id = allocation_wallet.address();
        let indexer = address!("1234567890123456789012345678901234567890");
        let subgraph_service = address!("94dc3B65AF05a7A8d36B877eb5DE68B6B16B6389");

        // Create the EIP-712 domain for SubgraphService
        let domain = subgraph_service_eip712_domain(42161, subgraph_service);

        // Create the allocation proof struct
        let proof_data = AllocationIdProof {
            indexer,
            allocationId: allocation_id,
        };

        // Compute the EIP-712 signing hash
        let signing_hash = proof_data.eip712_signing_hash(&domain);

        // Sign the hash using the allocation wallet
        let signature = allocation_wallet.sign_hash(&signing_hash).await.unwrap();

        // Verify we can recover the signer from the signature
        let recovered = signature
            .recover_address_from_prehash(&signing_hash)
            .expect("should recover signer");

        // The recovered signer should be the allocation ID (allocation wallet's address)
        // This is exactly what the SubgraphService contract verifies
        assert_eq!(
            recovered, allocation_id,
            "Recovered signer should equal allocation ID"
        );
    }

    #[tokio::test]
    async fn test_allocation_proof_signature_bytes() {
        use alloy::{
            primitives::address,
            signers::local::{coins_bip39::English, MnemonicBuilder, PrivateKeySigner},
            sol_types::SolStruct,
        };
        use thegraph_core::alloy::hex::ToHexExt;

        // Deterministic test vector: derived from a public test mnemonic.
        let allocation_wallet: PrivateKeySigner = MnemonicBuilder::<English>::default()
            .phrase(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            )
            .index(0u32)
            .unwrap()
            .build()
            .unwrap();
        let allocation_id = allocation_wallet.address();
        let indexer = address!("1234567890123456789012345678901234567890");
        let subgraph_service = address!("94dc3B65AF05a7A8d36B877eb5DE68B6B16B6389");
        let domain = subgraph_service_eip712_domain(42161, subgraph_service);

        let proof_data = AllocationIdProof {
            indexer,
            allocationId: allocation_id,
        };
        let signing_hash = proof_data.eip712_signing_hash(&domain);
        let signature = allocation_wallet.sign_hash(&signing_hash).await.unwrap();
        let signature_hex = signature.as_bytes().encode_hex();

        let expected = "61460d93990b5f8d08d8e82ac995be9f46e4433c3b8764e14d3d2a8565d85a0b6340a054d41c66b1ab272db68e3802fe687b6e2ef52f70fa5b3a6f855255a7a11c";
        assert_eq!(signature_hex, expected);
    }

    #[tokio::test]
    async fn test_resolve_poi_requires_block_number_when_explicit() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user@localhost/db")
            .expect("connect_lazy should not error");
        let provider_cache = Arc::new(ProviderCache::new("http://localhost:8545".to_string()));
        let signer = PrivateKeySigner::random();
        let config = ExecutorConfig {
            protocol_network: "eip155:1".to_string(),
            chain_id: 1,
            ..ExecutorConfig::default()
        };
        let (_epoch_tx, epoch_rx) = watch::channel(0u64);
        let executor = ActionExecutor::new(pool, provider_cache, signer, config, epoch_rx);

        let action = Action {
            id: 1,
            action_type: ActionType::Unallocate,
            status: ActionStatus::Queued,
            priority: Some(0),
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: Some(
                allocation_id!("1234567890123456789012345678901234567890").to_string(),
            ),
            amount: None,
            poi: Some(
                "0x0000000000000000000000000000000000000000000000000000000000000001".to_string(),
            ),
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            transaction: None,
            unallocate_transaction: None,
            failure_reason: None,
            protocol_network: "eip155:1".to_string(),
            is_legacy: false,
            public_poi: None,
            poi_block_number: None,
            created_at: None,
            updated_at: None,
        };

        let deployment_id: DeploymentId = action.deployment_id.parse().unwrap();
        let err = executor
            .resolve_poi_for_action(&action, &deployment_id)
            .await
            .unwrap_err();

        match err {
            ExecutorError::TransactionBuild(msg) => {
                assert!(msg.contains("missing POI block number"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_poi_explicit_bypasses_resolver() {
        use alloy::primitives::B256;

        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user@localhost/db")
            .expect("connect_lazy should not error");
        let provider_cache = Arc::new(ProviderCache::new("http://localhost:8545".to_string()));
        let signer = PrivateKeySigner::random();
        let config = ExecutorConfig {
            protocol_network: "eip155:1".to_string(),
            chain_id: 1,
            ..ExecutorConfig::default()
        };
        let (_epoch_tx, epoch_rx) = watch::channel(0u64);
        let executor = ActionExecutor::new(pool, provider_cache, signer, config, epoch_rx);

        let action = Action {
            id: 1,
            action_type: ActionType::Unallocate,
            status: ActionStatus::Queued,
            priority: Some(0),
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: Some(
                allocation_id!("1234567890123456789012345678901234567890").to_string(),
            ),
            amount: None,
            poi: Some(
                "0x00000000000000000000000000000000000000000000000000000000000000aa".to_string(),
            ),
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            transaction: None,
            unallocate_transaction: None,
            failure_reason: None,
            protocol_network: "eip155:1".to_string(),
            is_legacy: false,
            public_poi: Some(
                "0x00000000000000000000000000000000000000000000000000000000000000bb".to_string(),
            ),
            poi_block_number: Some(123),
            created_at: None,
            updated_at: None,
        };

        let deployment_id: DeploymentId = action.deployment_id.parse().unwrap();
        let (poi, block, public_poi) = executor
            .resolve_poi_for_action(&action, &deployment_id)
            .await
            .expect("explicit POI should resolve");

        let expected_poi =
            ProofOfIndexing::from(action.poi.as_ref().unwrap().parse::<B256>().unwrap());
        let expected_public =
            ProofOfIndexing::from(action.public_poi.as_ref().unwrap().parse::<B256>().unwrap());

        assert_eq!(poi, expected_poi);
        assert_eq!(block, BlockNumber::from(123u64));
        assert_eq!(public_poi, Some(expected_public));
    }
}
