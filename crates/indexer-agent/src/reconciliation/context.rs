// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Reconciliation context containing all state needed for the reconciliation loop.

use std::collections::HashMap;

use sqlx::PgPool;
use thegraph_core::{alloy::primitives::Address, IndexerId};

use crate::{models::IndexingRule, rules::PreprocessedRules};

/// Context for a single reconciliation cycle.
///
/// Contains all the data needed to make allocation decisions and queue actions.
#[derive(Debug)]
pub struct ReconciliationContext {
    /// Database connection pool for queuing actions
    pub pool: PgPool,
    /// The indexer's address
    pub indexer_id: IndexerId,
    /// Protocol network identifier (CAIP-2 format, e.g., "eip155:1")
    pub protocol_network: String,
    /// Current epoch number from the network
    pub current_epoch: u64,
    /// Maximum allocation lifetime in epochs
    pub max_allocation_epochs: u64,
    /// Whether to auto-approve actions (AUTO mode) or queue them (OVERSIGHT mode)
    pub auto_approve: bool,
    /// Cooldown period in seconds before re-queueing actions for the same deployment
    pub action_cooldown_secs: u64,
}

impl ReconciliationContext {
    /// Create a new reconciliation context.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        indexer_id: IndexerId,
        protocol_network: String,
        current_epoch: u64,
        max_allocation_epochs: u64,
        auto_approve: bool,
        action_cooldown_secs: u64,
    ) -> Self {
        Self {
            pool,
            indexer_id,
            protocol_network,
            current_epoch,
            max_allocation_epochs,
            auto_approve,
            action_cooldown_secs,
        }
    }

    /// Load and preprocess indexing rules from the database.
    pub async fn load_rules(&self) -> Result<PreprocessedRules, sqlx::Error> {
        let rules = IndexingRule::get_all(&self.pool, &self.protocol_network).await?;
        Ok(PreprocessedRules::from_rules(&rules))
    }
}

/// Deployment data fetched from the network subgraph.
///
/// This is a simplified version for reconciliation that contains
/// just the fields needed for allocation decisions.
#[derive(Debug, Clone)]
pub struct NetworkDeploymentData {
    /// Deployment ID (bytes32 format)
    pub id: String,
    /// IPFS hash
    pub ipfs_hash: String,
    /// Block at which the deployment was denied (0 if not denied)
    pub denied_at: i64,
    /// Total GRT staked on this deployment
    pub staked_tokens: bigdecimal::BigDecimal,
    /// Total GRT signalled on this deployment
    pub signalled_tokens: bigdecimal::BigDecimal,
    /// Total query fees earned
    pub query_fees_amount: bigdecimal::BigDecimal,
}

impl From<NetworkDeploymentData> for crate::rules::NetworkDeployment {
    fn from(data: NetworkDeploymentData) -> Self {
        Self {
            id: data.id,
            ipfs_hash: data.ipfs_hash,
            denied_at: data.denied_at,
            staked_tokens: data.staked_tokens,
            signalled_tokens: data.signalled_tokens,
            query_fees_amount: data.query_fees_amount,
        }
    }
}

/// Active allocation data from the network subgraph.
#[derive(Debug, Clone)]
pub struct ActiveAllocation {
    /// Allocation ID
    pub id: Address,
    /// Deployment ID this allocation is for
    pub deployment_id: String,
    /// Amount of GRT allocated
    pub allocated_tokens: String,
    /// Epoch when the allocation was created
    pub created_at_epoch: u64,
    /// Whether this is a legacy (V1) allocation
    pub is_legacy: bool,
}

/// Maps deployment ID to active allocations for that deployment.
pub type AllocationsByDeployment = HashMap<String, Vec<ActiveAllocation>>;
