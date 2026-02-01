// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! TAP Agent integration for the unified binary.
//!
//! This module handles starting the TAP agent actors within the service process,
//! enabling direct receipt notifications through channels instead of pg_notify.

use indexer_config::Config;
use indexer_monitor::{EscrowAccounts, SubgraphClient};
use indexer_tap_agent::agent::{
    sender_account::SenderAccountConfig,
    sender_accounts_manager::{ChannelReceiptNotification, SenderAccountsManagerMessage},
    start_agent_with_deps, StartAgentArgs,
};
use ractor::ActorRef;
use sqlx::PgPool;
use thegraph_core::alloy::{primitives::Address, sol_types::Eip712Domain};
use tokio::sync::{mpsc, watch::Receiver};

/// Handle to the running TAP agent.
///
/// This is returned when the TAP agent is started and can be used to:
/// - Send receipt notifications through the channel
/// - Gracefully shut down the agent
pub struct TapAgentHandle {
    /// Actor reference for the SenderAccountsManager
    pub manager: ActorRef<SenderAccountsManagerMessage>,
    /// Join handle for the actor's background task
    pub handle: ractor::concurrency::JoinHandle<()>,
    /// Channel sender for receipt notifications
    pub notification_tx: mpsc::Sender<ChannelReceiptNotification>,
}

impl TapAgentHandle {
    /// Gracefully shut down the TAP agent.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        use ractor::ActorStatus;

        // Drop the notification sender to signal the channel watcher to stop
        drop(self.notification_tx);

        // Kill the manager if it's still running
        if self.manager.get_status() == ActorStatus::Running {
            self.manager.kill_and_wait(None).await?;
        }

        // Wait for the actor to fully stop
        self.handle.await?;

        Ok(())
    }
}

/// Start the TAP agent with shared resources from the service.
///
/// This creates the TAP agent actors and returns a handle that can be used to
/// send receipt notifications and coordinate shutdown.
///
/// # Arguments
///
/// * `config` - Service configuration
/// * `pgpool` - Database connection pool (shared with service)
/// * `network_subgraph` - Network subgraph client
/// * `escrow_subgraph` - Escrow subgraph client
/// * `escrow_accounts_v2` - V2 escrow accounts watcher
/// * `domain_separator_v2` - EIP-712 domain separator for V2
/// * `is_horizon_enabled` - Whether Horizon mode is active
#[allow(clippy::too_many_arguments)]
pub async fn start_tap_agent(
    config: &Config,
    pgpool: PgPool,
    network_subgraph: &'static SubgraphClient,
    escrow_subgraph: &'static SubgraphClient,
    escrow_accounts_v2: Receiver<EscrowAccounts>,
    domain_separator_v2: Eip712Domain,
    is_horizon_enabled: bool,
) -> anyhow::Result<TapAgentHandle> {
    // Create the indexer allocations watcher (required by TAP agent)
    let indexer_allocations = indexer_monitor::indexer_allocations(
        network_subgraph,
        config.indexer.indexer_address,
        config.subgraphs.network.config.syncing_interval_secs,
        config
            .subgraphs
            .network
            .recently_closed_allocation_buffer_secs,
    )
    .await?;

    // Create sender account config from the service config
    let sender_account_config = SenderAccountConfig::from_config(config);

    // Create channel for receipt notifications
    // Use a bounded channel to provide backpressure if the agent falls behind
    let (notification_tx, notification_rx) = mpsc::channel(10_000);

    // Build the args for the TAP agent
    let args = StartAgentArgs {
        pgpool,
        network_subgraph,
        escrow_subgraph,
        indexer_allocations,
        escrow_accounts_v2,
        domain_separator_v2,
        config: sender_account_config,
        sender_aggregator_endpoints: config.tap.sender_aggregator_endpoints.clone(),
        is_horizon_enabled,
        prefix: None,
        receipt_notification_rx: notification_rx,
    };

    // Initialize TAP agent metrics
    indexer_tap_agent::agent::init_metrics();

    // Start the TAP agent
    let (manager, handle) = start_agent_with_deps(args).await?;

    tracing::info!("TAP agent started successfully (unified binary mode)");

    Ok(TapAgentHandle {
        manager,
        handle,
        notification_tx,
    })
}

/// Create a receipt notification from V2 data.
pub fn create_v2_notification(
    id: u64,
    collection_id: String,
    signer_address: Address,
    timestamp_ns: u64,
    value: u128,
) -> ChannelReceiptNotification {
    use indexer_tap_agent::agent::sender_accounts_manager::NewReceiptNotification;

    ChannelReceiptNotification {
        notification: NewReceiptNotification {
            id,
            collection_id,
            signer_address,
            timestamp_ns,
            value,
        },
    }
}
