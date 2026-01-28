// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Main reconciliation loop task.
//!
//! Runs as a background tokio task that periodically reconciles
//! the indexer's allocation state with the desired state from rules.

use std::time::Duration;

use sqlx::PgPool;
use thegraph_core::alloy::primitives::Address;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use super::{
    context::{ActiveAllocation, NetworkDeploymentData, ReconciliationContext},
    deployments::reconcile_deployment_allocations,
};
use crate::{
    models::Action,
    rules::{evaluate_deployments, NetworkDeployment},
};

/// Default cooldown period in seconds (15 minutes, matching TypeScript agent).
pub const DEFAULT_ACTION_COOLDOWN_SECS: u64 = 900;

/// Configuration for the reconciliation loop.
#[derive(Debug, Clone)]
pub struct ReconciliationConfig {
    /// Interval between reconciliation cycles
    pub interval: Duration,
    /// Protocol network identifier (CAIP-2 format)
    pub protocol_network: String,
    /// Maximum allocation lifetime in epochs
    pub max_allocation_epochs: u64,
    /// Whether to auto-approve actions (AUTO mode)
    pub auto_approve: bool,
    /// Cooldown period in seconds before re-queueing actions for the same deployment.
    ///
    /// After an action completes (success or failure), new actions for the same
    /// deployment will be blocked until this cooldown period expires. This prevents
    /// rapid action cycling that wastes gas.
    ///
    /// Default: 900 seconds (15 minutes), matching the TypeScript agent.
    pub action_cooldown_secs: u64,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(120), // 2 minutes
            protocol_network: "eip155:1".to_string(),
            max_allocation_epochs: 28,
            auto_approve: false,
            action_cooldown_secs: DEFAULT_ACTION_COOLDOWN_SECS,
        }
    }
}

/// Run the reconciliation loop.
///
/// This function runs indefinitely, periodically reconciling the indexer's
/// allocation state with the desired state from rules.
///
/// # Arguments
///
/// * `pool` - Database connection pool
/// * `indexer_address` - The indexer's address
/// * `config` - Reconciliation configuration
/// * `deployments_rx` - Watch receiver for network deployment data
/// * `allocations_rx` - Watch receiver for active allocations
/// * `epoch_rx` - Watch receiver for current epoch number
/// * `shutdown_rx` - Watch receiver for shutdown signal
pub async fn run_reconciliation_loop(
    pool: PgPool,
    indexer_address: Address,
    config: ReconciliationConfig,
    deployments_rx: watch::Receiver<Vec<NetworkDeploymentData>>,
    allocations_rx: watch::Receiver<Vec<ActiveAllocation>>,
    epoch_rx: watch::Receiver<u64>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    info!(
        interval_secs = config.interval.as_secs(),
        protocol_network = %config.protocol_network,
        auto_approve = config.auto_approve,
        "Starting reconciliation loop"
    );

    let mut interval = tokio::time::interval(config.interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = run_reconciliation_cycle(
                    &pool,
                    indexer_address,
                    &config,
                    &deployments_rx,
                    &allocations_rx,
                    &epoch_rx,
                ).await {
                    error!(error = %e, "Reconciliation cycle failed");
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown signal received, stopping reconciliation loop");
                    break;
                }
            }
        }
    }
}

/// Run a single reconciliation cycle.
async fn run_reconciliation_cycle(
    pool: &PgPool,
    indexer_address: Address,
    config: &ReconciliationConfig,
    deployments_rx: &watch::Receiver<Vec<NetworkDeploymentData>>,
    allocations_rx: &watch::Receiver<Vec<ActiveAllocation>>,
    epoch_rx: &watch::Receiver<u64>,
) -> Result<(), anyhow::Error> {
    debug!("Starting reconciliation cycle");

    // Check for approved actions awaiting execution
    let approved_actions = Action::get_approved(pool, &config.protocol_network).await?;
    if !approved_actions.is_empty() {
        info!(
            count = approved_actions.len(),
            "Skipping reconciliation: approved actions awaiting execution"
        );
        return Ok(());
    }

    // Get current state from watchers
    let deployments = deployments_rx.borrow().clone();
    let allocations = allocations_rx.borrow().clone();
    let current_epoch = *epoch_rx.borrow();

    if deployments.is_empty() {
        warn!("No network deployments available, skipping reconciliation");
        return Ok(());
    }

    info!(
        deployments = deployments.len(),
        allocations = allocations.len(),
        epoch = current_epoch,
        "Running reconciliation cycle"
    );

    // Create context for this cycle
    let ctx = ReconciliationContext::new(
        pool.clone(),
        indexer_address,
        config.protocol_network.clone(),
        current_epoch,
        config.max_allocation_epochs,
        config.auto_approve,
        config.action_cooldown_secs,
    );

    // Load and preprocess rules
    let rules = ctx.load_rules().await?;

    // Convert deployment data to evaluation format
    let network_deployments: Vec<NetworkDeployment> =
        deployments.into_iter().map(|d| d.into()).collect();

    // Evaluate deployments against rules
    let decisions = evaluate_deployments(&network_deployments, &rules);

    let to_allocate = decisions.iter().filter(|d| d.to_allocate).count();
    let to_skip = decisions.len() - to_allocate;
    debug!(
        to_allocate = to_allocate,
        to_skip = to_skip,
        "Evaluated deployments"
    );

    // Reconcile allocations
    let results = reconcile_deployment_allocations(&ctx, &decisions, &allocations, &rules).await?;

    let actions_queued: usize = results.iter().map(|r| r.actions_queued.len()).sum();
    if actions_queued > 0 {
        info!(
            actions_queued = actions_queued,
            "Reconciliation cycle complete"
        );
    } else {
        debug!("Reconciliation cycle complete, no actions needed");
    }

    Ok(())
}

/// Simplified reconciliation loop that uses static data (for testing).
///
/// This version doesn't require watch channels and can be used for
/// one-shot reconciliation or testing.
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_once(
    pool: &PgPool,
    indexer_address: Address,
    protocol_network: &str,
    current_epoch: u64,
    max_allocation_epochs: u64,
    auto_approve: bool,
    deployments: Vec<NetworkDeploymentData>,
    allocations: Vec<ActiveAllocation>,
) -> Result<Vec<Action>, anyhow::Error> {
    reconcile_once_with_cooldown(
        pool,
        indexer_address,
        protocol_network,
        current_epoch,
        max_allocation_epochs,
        auto_approve,
        DEFAULT_ACTION_COOLDOWN_SECS,
        deployments,
        allocations,
    )
    .await
}

/// Simplified reconciliation loop that uses static data (for testing).
///
/// This version includes explicit cooldown configuration.
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_once_with_cooldown(
    pool: &PgPool,
    indexer_address: Address,
    protocol_network: &str,
    current_epoch: u64,
    max_allocation_epochs: u64,
    auto_approve: bool,
    action_cooldown_secs: u64,
    deployments: Vec<NetworkDeploymentData>,
    allocations: Vec<ActiveAllocation>,
) -> Result<Vec<Action>, anyhow::Error> {
    // Check for approved actions awaiting execution
    let approved_actions = Action::get_approved(pool, protocol_network).await?;
    if !approved_actions.is_empty() {
        info!(
            count = approved_actions.len(),
            "Skipping reconciliation: approved actions awaiting execution"
        );
        return Ok(vec![]);
    }

    // Create context
    let ctx = ReconciliationContext::new(
        pool.clone(),
        indexer_address,
        protocol_network.to_string(),
        current_epoch,
        max_allocation_epochs,
        auto_approve,
        action_cooldown_secs,
    );

    // Load rules
    let rules = ctx.load_rules().await?;

    // Convert and evaluate
    let network_deployments: Vec<NetworkDeployment> =
        deployments.into_iter().map(|d| d.into()).collect();
    let decisions = evaluate_deployments(&network_deployments, &rules);

    // Reconcile
    let results = reconcile_deployment_allocations(&ctx, &decisions, &allocations, &rules).await?;

    // Collect all queued actions
    let actions: Vec<Action> = results.into_iter().flat_map(|r| r.actions_queued).collect();

    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReconciliationConfig::default();
        assert_eq!(config.interval, Duration::from_secs(120));
        assert_eq!(config.max_allocation_epochs, 28);
        assert!(!config.auto_approve);
        assert_eq!(config.action_cooldown_secs, DEFAULT_ACTION_COOLDOWN_SECS);
    }

    #[test]
    fn test_default_action_cooldown_is_fifteen_minutes() {
        // Verify the default matches TypeScript agent's 15-minute cooldown
        assert_eq!(DEFAULT_ACTION_COOLDOWN_SECS, 900);
    }
}
