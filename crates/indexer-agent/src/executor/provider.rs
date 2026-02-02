// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Provider cache for managing blockchain connections with nonce management.
//!
//! This module implements a provider cache that ensures all transactions from
//! the same signer on the same chain share a single nonce manager, preventing
//! nonce conflicts in concurrent execution.

use std::{collections::HashMap, sync::Arc};

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::Address,
    providers::{
        fillers::{FillProvider, JoinFill, WalletFiller},
        utils::JoinedRecommendedFillers,
        Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
};
use tokio::sync::RwLock;

use super::errors::ExecutorError;

/// Type alias for the provider with Alloy's recommended fillers.
///
/// This provider automatically handles:
/// - Nonce management (cached for sequential transactions)
/// - Chain ID filling
/// - Gas estimation
/// - Blob gas filling (for EIP-4844)
/// - Transaction signing
pub type ExecutorProvider = FillProvider<
    JoinFill<JoinedRecommendedFillers, WalletFiller<EthereumWallet>>,
    RootProvider<Ethereum>,
    Ethereum,
>;

/// Key for caching providers: (chain_id, signer_address).
///
/// All transactions from the same signer on the same chain must share
/// a provider to ensure sequential nonce management.
type ProviderKey = (u64, Address);

/// Cache for blockchain providers with nonce management.
///
/// This cache ensures that:
/// 1. Each (chain, signer) pair has exactly one provider
/// 2. Nonces are managed sequentially to prevent conflicts
/// 3. Providers can be reset on nonce errors to resync with the network
pub struct ProviderCache {
    /// Cached providers keyed by (chain_id, signer_address)
    providers: RwLock<HashMap<ProviderKey, Arc<ExecutorProvider>>>,

    /// RPC URL for the Ethereum node
    rpc_url: String,
}

impl ProviderCache {
    /// Create a new provider cache.
    ///
    /// # Arguments
    /// * `rpc_url` - URL of the Ethereum RPC endpoint
    pub fn new(rpc_url: String) -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            rpc_url,
        }
    }

    /// Get or create a provider for the given chain and signer.
    ///
    /// If a provider already exists for this (chain, signer) pair, it will be
    /// reused to maintain nonce continuity. Otherwise, a new provider is created.
    ///
    /// # Arguments
    /// * `chain_id` - The chain ID
    /// * `signer` - The private key signer
    pub async fn get_provider(
        &self,
        chain_id: u64,
        signer: PrivateKeySigner,
    ) -> Result<Arc<ExecutorProvider>, ExecutorError> {
        let signer_address = signer.address();
        let key = (chain_id, signer_address);

        // Check if we have a cached provider
        {
            let providers = self.providers.read().await;
            if let Some(provider) = providers.get(&key) {
                return Ok(Arc::clone(provider));
            }
        }

        // Create a new provider
        let provider = self.create_provider(signer).await?;
        let provider = Arc::new(provider);

        // Cache it
        {
            let mut providers = self.providers.write().await;
            providers.insert(key, Arc::clone(&provider));
        }

        Ok(provider)
    }

    /// Reset the provider for a given chain and signer.
    ///
    /// This should be called when a nonce error is detected to force
    /// the creation of a fresh provider that will fetch the current
    /// nonce from the network.
    ///
    /// # Arguments
    /// * `chain_id` - The chain ID
    /// * `signer_address` - The signer's address
    pub async fn reset_provider(&self, chain_id: u64, signer_address: Address) {
        let key = (chain_id, signer_address);
        let mut providers = self.providers.write().await;
        providers.remove(&key);
        tracing::info!(
            chain_id = chain_id,
            signer = %signer_address,
            "Reset provider cache entry due to nonce error"
        );
    }

    /// Create a new provider with all fillers configured.
    async fn create_provider(
        &self,
        signer: PrivateKeySigner,
    ) -> Result<ExecutorProvider, ExecutorError> {
        let wallet = EthereumWallet::from(signer);
        let rpc_url = self
            .rpc_url
            .parse()
            .map_err(|e| ExecutorError::Config(format!("invalid RPC URL: {e}")))?;

        let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc_url);

        Ok(provider)
    }

    /// Get the chain ID from the RPC endpoint.
    pub async fn get_chain_id(&self) -> Result<u64, ExecutorError> {
        let rpc_url = self
            .rpc_url
            .parse()
            .map_err(|e| ExecutorError::Config(format!("invalid RPC URL: {e}")))?;

        let provider = ProviderBuilder::new().connect_http(rpc_url);

        provider
            .get_chain_id()
            .await
            .map_err(|e| ExecutorError::Provider(format!("failed to get chain ID: {e}")))
    }

    /// Check if the signer has sufficient balance for gas costs.
    ///
    /// # Arguments
    /// * `provider` - The provider to use
    /// * `signer_address` - The signer's address
    /// * `required_gas` - The estimated gas cost in wei
    pub async fn check_gas_balance(
        provider: &ExecutorProvider,
        signer_address: Address,
        required_gas: alloy::primitives::U256,
    ) -> Result<(), ExecutorError> {
        let balance = provider
            .get_balance(signer_address)
            .await
            .map_err(|e| ExecutorError::Provider(format!("failed to get balance: {e}")))?;

        if balance < required_gas {
            return Err(ExecutorError::InsufficientGasBalance {
                available: balance.to_string(),
                required: required_gas.to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_key() {
        let chain_id = 42161u64;
        let address = Address::ZERO;
        let key: ProviderKey = (chain_id, address);
        assert_eq!(key.0, 42161);
        assert_eq!(key.1, Address::ZERO);
    }
}
