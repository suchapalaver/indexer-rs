// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    sync::LazyLock,
    time::Duration,
};

use anyhow::{anyhow, bail};
use futures::{stream, StreamExt};
use indexer_allocation::Allocation;
use indexer_monitor::{EscrowAccounts, SubgraphClient};
use indexer_watcher::{map_watcher, watch_pipe};
use prometheus::{register_counter_vec, CounterVec};
use ractor::{Actor, ActorCell, ActorProcessingErr, ActorRef, SupervisionEvent};
use reqwest::Url;
use serde::Deserialize;
use sqlx::PgPool;
use thegraph_core::{
    alloy::{hex::ToHexExt, primitives::Address, sol_types::Eip712Domain},
    CollectionId,
};
use tokio::{select, sync::watch::Receiver};

use super::sender_account::{
    SenderAccount, SenderAccountArgs, SenderAccountConfig, SenderAccountMessage,
};
use crate::agent::sender_allocation::SenderAllocationMessage;

pub(crate) static RECEIPTS_CREATED: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "tap_receipts_received_total",
        "Receipts received since start of the program.",
        &["sender", "allocation"]
    )
    .unwrap()
});

const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Notification received by pgnotify for Horizon (V2) receipts
///
/// This contains a list of properties that are sent by postgres when a V2 receipt is inserted
#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct NewReceiptNotification {
    /// id inside the table
    pub id: u64,
    /// collection id (V2 uses 32-byte collection_id)
    pub collection_id: String, // 64-character hex string from database
    /// address of wallet that signed this receipt
    pub signer_address: Address,
    /// timestamp of the receipt
    pub timestamp_ns: u64,
    /// value of the receipt
    pub value: u128,
}

impl NewReceiptNotification {
    /// Get the ID regardless of version
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Get the signer address regardless of version
    pub fn signer_address(&self) -> Address {
        self.signer_address
    }

    /// Get the timestamp regardless of version
    pub fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    /// Get the value regardless of version
    pub fn value(&self) -> u128 {
        self.value
    }

    /// Get the collection ID as a unified type
    #[tracing::instrument(skip(self), ret)]
    pub fn collection_id(&self) -> CollectionId {
        // Convert the hex string to CollectionId (trim spaces from fixed-length DB field)
        let trimmed = self.collection_id.trim();
        match CollectionId::from_str(trimmed) {
            Ok(collection_id) => collection_id,
            Err(e) => {
                // Check if this is a 20-byte address (40 hex chars) from migration period
                // TRST-L-9: Always route V2 receipts to Horizon, never downgrade to Legacy
                let hex_str = trimmed.strip_prefix("0x").unwrap_or(trimmed);
                if hex_str.len() == 64 {
                    match CollectionId::from_str(&format!("0x{hex_str}")) {
                        Ok(collection_id) => return collection_id,
                        Err(hex_err) => {
                            tracing::error!(
                                collection_id = %self.collection_id,
                                error = %hex_err,
                                "Failed to parse 32-byte collection_id with 0x prefix"
                            );
                        }
                    }
                }
                if hex_str.len() == 40 {
                    // 20-byte address during migration - convert to CollectionId
                    match Address::from_str(&format!("0x{hex_str}")) {
                        Ok(address) => {
                            tracing::debug!(
                                collection_id = %self.collection_id,
                                address = %address,
                                "Converting 20-byte address to CollectionId for V2 receipt"
                            );
                            return CollectionId::from(address);
                        }
                        Err(addr_err) => {
                            tracing::error!(
                                collection_id = %self.collection_id,
                                error = %addr_err,
                                "Failed to parse 20-byte address"
                            );
                        }
                    }
                } else {
                    tracing::error!(
                        collection_id = %self.collection_id,
                        hex_len = hex_str.len(),
                        error = %e,
                        "Failed to parse collection_id from database notification"
                    );
                }
                // Fallback: use zero CollectionId but stay on Horizon path
                CollectionId::from(Address::ZERO)
            }
        }
    }
}

/// Manager Actor
#[derive(Debug, Clone)]
pub struct SenderAccountsManager;

/// Canonical hex (no 0x); 64 chars for Horizon.
fn collection_id_hex(collection_id: &CollectionId) -> String {
    format!("{:x}", collection_id)
}

/// Type used in [SenderAccountsManager] and [SenderAccount] to route Horizon-specific logic.
#[derive(Clone, Copy, Debug)]
pub enum SenderType {
    /// SenderAccounts that are found in Tap Collector v2 (Horizon)
    Horizon,
}

/// Enum containing all types of messages that a [SenderAccountsManager] can receive
#[derive(Debug)]
#[cfg_attr(any(test, feature = "test"), derive(Clone))]
pub enum SenderAccountsManagerMessage {
    /// Spawn and Stop [SenderAccount]s that were added or removed
    /// in comparison with it current state and updates the state
    ///
    /// This tracks only v2 accounts
    UpdateSenderAccountsV2(HashSet<Address>),
}

/// Receipt notification sent through a channel from the service.
///
/// TAP agent consumes these notifications directly (channel-only path).
#[derive(Debug, Clone)]
pub struct ChannelReceiptNotification {
    /// The receipt notification (V1 or V2)
    pub notification: NewReceiptNotification,
}

/// Arguments received in startup while spawing [SenderAccount] actor
pub struct SenderAccountsManagerArgs {
    /// Config forwarded to [SenderAccount]
    pub config: &'static SenderAccountConfig,

    /// Domain separator used for tap v2 (Horizon)
    pub domain_separator_v2: Eip712Domain,

    /// Database connection
    pub pgpool: PgPool,
    /// Watcher that returns a map of open and recently closed allocation ids
    pub indexer_allocations: Receiver<HashMap<Address, Allocation>>,
    /// Watcher containing the escrow accounts for v2
    pub escrow_accounts_v2: Receiver<EscrowAccounts>,
    /// SubgraphClient of the network subgraph
    pub network_subgraph: &'static SubgraphClient,
    /// Map containing all endpoints for senders provided in the config
    pub sender_aggregator_endpoints: HashMap<Address, Url>,

    /// Prefix used to bypass limitations of global actor registry (used for tests)
    pub prefix: Option<String>,

    /// Channel receiver for receipt notifications from the service.
    ///
    /// The TAP agent no longer listens to pg_notify; all receipt notifications
    /// must arrive through this channel.
    pub receipt_notification_rx: tokio::sync::mpsc::Receiver<ChannelReceiptNotification>,
}

/// State for [SenderAccountsManager] actor
///
/// This is a separate instance that makes it easier to have mutable
/// reference, for more information check ractor library
pub struct State {
    sender_ids_v2: HashSet<Address>,
    /// Handle for the channel-based receipt notification watcher
    channel_receipts_watcher_handle: Option<tokio::task::JoinHandle<()>>,

    config: &'static SenderAccountConfig,
    domain_separator_v2: Eip712Domain,
    pgpool: PgPool,
    // Raw allocation watcher (address -> Allocation). Normalized per-sender later.
    indexer_allocations: Receiver<HashMap<Address, Allocation>>,
    /// Watcher containing the escrow accounts for v2
    escrow_accounts_v2: Receiver<EscrowAccounts>,
    network_subgraph: &'static SubgraphClient,
    sender_aggregator_endpoints: HashMap<Address, Url>,
    prefix: Option<String>,
}

#[async_trait::async_trait]
impl Actor for SenderAccountsManager {
    type Msg = SenderAccountsManagerMessage;
    type State = State;
    type Arguments = SenderAccountsManagerArgs;

    /// This is called in the [ractor::Actor::spawn] method and is used
    /// to process the [SenderAccountsManagerArgs] with a reference to the current
    /// actor
    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        SenderAccountsManagerArgs {
            config,
            domain_separator_v2,
            indexer_allocations,
            pgpool,
            escrow_accounts_v2,
            network_subgraph,
            sender_aggregator_endpoints,
            prefix,
            receipt_notification_rx,
        }: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        // Do not pre-map allocations globally. We keep the raw watcher and
        // normalize per SenderAccount based on Horizon collection ids.
        tracing::info!(
            horizon_active = %config.tap_mode.is_horizon(),
            "Using raw indexer_allocations watcher; normalization happens per sender"
        );
        let myself_clone = myself.clone();
        let _escrow_accounts_v2 = escrow_accounts_v2.clone();
        watch_pipe(_escrow_accounts_v2, move |escrow_accounts| {
            let senders = escrow_accounts.get_senders();
            myself_clone
                .cast(SenderAccountsManagerMessage::UpdateSenderAccountsV2(
                    senders,
                ))
                .unwrap_or_else(|e| {
                    tracing::error!(error = ?e, "Error while updating sender_accounts v2");
                });
            async {}
        });

        let mut state = State {
            config,
            domain_separator_v2,
            sender_ids_v2: HashSet::new(),
            channel_receipts_watcher_handle: None,
            pgpool: pgpool.clone(),
            indexer_allocations,
            escrow_accounts_v2: escrow_accounts_v2.clone(),
            network_subgraph,
            sender_aggregator_endpoints,
            prefix: prefix.clone(),
        };
        let sender_allocation_v2 = select! {
            sender_allocation = state.get_pending_sender_allocation_id_v2() => sender_allocation,
            _ = tokio::time::sleep(state.config.tap_sender_timeout) => {
                panic!("Timeout while getting pending sender allocation ids");
            }
        };

        state.sender_ids_v2.extend(sender_allocation_v2.keys());
        stream::iter(sender_allocation_v2)
            .map(|(sender_id, allocation_ids)| {
                state.create_or_deny_sender(
                    myself.get_cell(),
                    sender_id,
                    allocation_ids,
                    SenderType::Horizon,
                )
            })
            .buffer_unordered(10) // Limit concurrency to 10 senders at a time
            .collect::<Vec<()>>()
            .await;

        tracing::info!("Starting channel-based receipt notification watcher");
        state.channel_receipts_watcher_handle = Some(tokio::spawn(channel_receipts_watcher(
            myself.get_cell(),
            receipt_notification_rx,
            state.escrow_accounts_v2.clone(),
            state.prefix.clone(),
        )));

        tracing::info!("SenderAccountManager created!");
        Ok(state)
    }

    async fn post_stop(
        &self,
        _: ActorRef<Self::Msg>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        // Abort the notification watchers on drop. Otherwise they may panic because the PgPool
        // could get dropped before. (Observed in tests)
        if let Some(handle) = &state.channel_receipts_watcher_handle {
            handle.abort();
        }

        Ok(())
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        tracing::trace!(
            message = ?msg,
            "New SenderAccountManager message"
        );

        match msg {
            SenderAccountsManagerMessage::UpdateSenderAccountsV2(target_senders) => {
                // Create new sender accounts
                for sender in target_senders.difference(&state.sender_ids_v2) {
                    state
                        .create_or_deny_sender(
                            myself.get_cell(),
                            *sender,
                            HashSet::new(),
                            SenderType::Horizon,
                        )
                        .await;
                }

                // Remove sender accounts
                for sender in state.sender_ids_v2.difference(&target_senders) {
                    if let Some(sender_handle) = ActorRef::<SenderAccountMessage>::where_is(
                        state.format_sender_account(sender, SenderType::Horizon),
                    ) {
                        sender_handle.stop(None);
                    }
                }

                state.sender_ids_v2 = target_senders;
            }
        }
        Ok(())
    }

    // we define the supervisor event to overwrite the default behavior which
    // is shutdown the supervisor on actor termination events
    async fn handle_supervisor_evt(
        &self,
        myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(cell, _, reason) => {
                let sender_id = cell.get_name();
                tracing::info!(?sender_id, ?reason, "Actor SenderAccount was terminated")
            }
            SupervisionEvent::ActorFailed(cell, error) => {
                let sender_id = cell.get_name();
                tracing::warn!(
                    ?sender_id,
                    ?error,
                    "Actor SenderAccount failed. Restarting..."
                );
                let Some(sender_id) = cell.get_name() else {
                    tracing::error!("SenderAllocation doesn't have a name");
                    return Ok(());
                };
                let mut splitter = sender_id.split(':');
                let Some(sender_id) = splitter.next_back() else {
                    tracing::error!(%sender_id, "Could not extract sender_id from name");
                    return Ok(());
                };
                let Ok(sender_id) = Address::parse_checksummed(sender_id, None) else {
                    tracing::error!(%sender_id, "Could not convert sender_id to Address");
                    return Ok(());
                };
                let sender_type = SenderType::Horizon;

                let allocations = {
                    let mut sender_allocation = select! {
                        sender_allocation = state.get_pending_sender_allocation_id_v2() => sender_allocation,
                        _ = tokio::time::sleep(state.config.tap_sender_timeout) => {
                            tracing::error!(version = "V2", "Timeout while getting pending sender allocation ids");
                            return Ok(());
                        }
                    };
                    sender_allocation
                        .remove(&sender_id)
                        .unwrap_or(HashSet::new())
                };

                state
                    .create_or_deny_sender(myself.get_cell(), sender_id, allocations, sender_type)
                    .await;
            }
            _ => {}
        }
        Ok(())
    }
}

impl State {
    fn format_sender_account(&self, sender: &Address, sender_type: SenderType) -> String {
        let mut sender_allocation_id = String::new();
        if let Some(prefix) = &self.prefix {
            sender_allocation_id.push_str(prefix);
            sender_allocation_id.push(':');
        }
        sender_allocation_id.push_str(match sender_type {
            SenderType::Horizon => "horizon:",
        });
        sender_allocation_id.push_str(&format!("{sender}"));
        sender_allocation_id
    }

    /// Helper function to create a [SenderAccount]
    ///
    /// It takes the current [SenderAccountsManager] cell to use it
    /// as supervisor, sender address and a list of initial allocations
    ///
    /// In case there's an error creating it, deny so it
    /// can no longer send queries
    async fn create_or_deny_sender(
        &self,
        supervisor: ActorCell,
        sender_id: Address,
        allocation_ids: HashSet<CollectionId>,
        sender_type: SenderType,
    ) {
        tracing::info!(
            sender = %sender_id,
            sender_type = ?sender_type,
            initial_allocations = allocation_ids.len(),
            "Creating SenderAccount",
        );
        for alloc_id in &allocation_ids {
            tracing::debug!(
                allocation_id = %alloc_id,
                address = %alloc_id.as_address(),
                "Initial allocation",
            );
        }

        if let Err(e) = self
            .create_sender_account(supervisor, sender_id, allocation_ids, sender_type)
            .await
        {
            tracing::error!(
                "There was an error while starting the sender {}, denying it. Error: {:?}",
                sender_id,
                e
            );
            SenderAccount::deny_sender(sender_type, &self.pgpool, sender_id).await;
        }
    }

    /// Helper function to create a [SenderAccount]
    ///
    /// It takes the current [SenderAccountsManager] cell to use it
    /// as supervisor, sender address and a list of initial allocations
    ///
    async fn create_sender_account(
        &self,
        supervisor: ActorCell,
        sender_id: Address,
        allocation_ids: HashSet<CollectionId>,
        sender_type: SenderType,
    ) -> anyhow::Result<()> {
        let Ok(args) = self.new_sender_account_args(&sender_id, allocation_ids, sender_type) else {
            tracing::warn!(
                "Sender {} is not on your [tap.sender_aggregator_endpoints] list. \
                        \
                        This means that you don't recognize this sender and don't want to \
                        provide queries for it.
                        \
                        If you do recognize and want to serve queries for it, \
                        add a new entry to the config [tap.sender_aggregator_endpoints]",
                sender_id
            );
            bail!(
                "No sender_aggregator_endpoints found for sender {}",
                sender_id
            );
        };
        SenderAccount::spawn_linked(
            Some(self.format_sender_account(&sender_id, sender_type)),
            SenderAccount,
            args,
            supervisor,
        )
        .await?;
        Ok(())
    }

    /// Gather all outstanding receipts and unfinalized RAVs from the database.
    /// Used to create [SenderAccount] instances for all senders that have unfinalized allocations
    /// and try to finalize them if they have become ineligible.
    ///
    /// This loads horizon allocations
    async fn get_pending_sender_allocation_id_v2(&self) -> HashMap<Address, HashSet<CollectionId>> {
        // First we accumulate all allocations for each sender. This is because we may have more
        // than one signer per sender in DB.
        let mut unfinalized_sender_allocations_map: HashMap<Address, HashSet<CollectionId>> =
            HashMap::new();

        let receipts_signer_collections_in_db = sqlx::query!(
            r#"
                WITH grouped AS (
                    SELECT signer_address, collection_id
                    FROM tap_horizon_receipts
                    WHERE data_service = $1 AND service_provider = $2
                    GROUP BY signer_address, collection_id
                )
                SELECT 
                    signer_address,
                    ARRAY_AGG(collection_id) AS collection_ids
                FROM grouped
                GROUP BY signer_address
            "#,
            self.config.tap_mode.subgraph_service_address.encode_hex(),
            self.config.indexer_address.encode_hex()
        )
        .fetch_all(&self.pgpool)
        .await
        .expect("should be able to fetch pending V2 receipts from the database");

        for row in receipts_signer_collections_in_db {
            let collection_ids =
                row.collection_ids
                    .expect("all receipts V2 should have a collection_id")
                    .iter()
                    .map(|collection_id| {
                        let trimmed = collection_id.trim();
                        let hex_str = if let Some(stripped) = trimmed.strip_prefix("0x") {
                            stripped
                        } else {
                            trimmed
                        };

                        // For migration period: collection_id in DB is actually a 20-byte address
                        // that needs to be converted to a 32-byte CollectionId
                        if hex_str.len() == 40 {
                            // 20-byte address -> convert to CollectionId using From<Address>
                            let address = Address::from_str(&format!("0x{hex_str}"))
                                .unwrap_or_else(|e| panic!("Invalid address '{trimmed}': {e}"));
                            CollectionId::from(address)
                        } else if hex_str.len() == 64 {
                            // 32-byte CollectionId
                            CollectionId::from_str(&format!("0x{hex_str}")).unwrap_or_else(|e| {
                                panic!("Invalid collection_id '{trimmed}': {e}")
                            })
                        } else {
                            panic!("Invalid collection_id length '{}': expected 40 or 64 hex characters, got {}", trimmed, hex_str.len())
                        }
                    })
                    .collect::<HashSet<_>>();
            let signer_id = Address::from_str(&row.signer_address)
                .expect("signer_address should be a valid address");
            let sender_id = self
                .escrow_accounts_v2
                .borrow()
                .get_sender_for_signer(&signer_id)
                .expect("should be able to get sender from signer");

            // Accumulate allocations for the sender
            unfinalized_sender_allocations_map
                .entry(sender_id)
                .or_default()
                .extend(collection_ids);
        }

        let nonfinal_ravs_sender_allocations_in_db = sqlx::query!(
            r#"
                SELECT
                    payer,
                    ARRAY_AGG(DISTINCT collection_id) FILTER (WHERE NOT last) AS allocation_ids
                FROM tap_horizon_ravs
                WHERE data_service = $1 AND service_provider = $2
                GROUP BY payer
            "#,
            // Constrain to our Horizon bucket to avoid conflating RAVs across services/providers
            self.config.tap_mode.subgraph_service_address.encode_hex(),
            self.config.indexer_address.encode_hex()
        )
        .fetch_all(&self.pgpool)
        .await
        .expect("should be able to fetch unfinalized V2 RAVs from the database");

        for row in nonfinal_ravs_sender_allocations_in_db {
            // Check if allocation_ids is Some before processing,
            // as ARRAY_AGG with FILTER returns NULL instead of an
            // empty array
            if let Some(allocation_id_strings) = row.allocation_ids {
                let allocation_ids = allocation_id_strings
                    .iter()
                    .map(|collection_id| {
                        let trimmed = collection_id.trim();
                        let hex_str = trimmed.strip_prefix("0x").unwrap_or(trimmed);
                        let prefixed = if hex_str.len() == 64 {
                            format!("0x{hex_str}")
                        } else {
                            trimmed.to_string()
                        };
                        CollectionId::from_str(&prefixed)
                            .expect("collection_id should be a valid collection ID")
                    })
                    .collect::<HashSet<_>>();

                if !allocation_ids.is_empty() {
                    let sender_id = Address::from_str(&row.payer)
                        .expect("sender_address should be a valid address");

                    unfinalized_sender_allocations_map
                        .entry(sender_id)
                        .or_default()
                        .extend(allocation_ids);
                }
            } else {
                // Log the case when allocation_ids is NULL
                tracing::warn!(
                    "Found NULL allocation_ids. This may indicate all RAVs are finalized."
                );
            }
        }
        unfinalized_sender_allocations_map
    }

    /// Helper function to create [SenderAccountArgs]
    ///
    /// Fails if the provided sender_id is not present
    /// in the sender_aggregator_endpoints map
    fn new_sender_account_args(
        &self,
        sender_id: &Address,
        allocation_ids: HashSet<CollectionId>,
        sender_type: SenderType,
    ) -> anyhow::Result<SenderAccountArgs> {
        let escrow_accounts = self.escrow_accounts_v2.clone();

        // Build a normalized allocation watcher for Horizon using the isLegacy flag
        // from the Network Subgraph (legacy allocations are ignored).
        let indexer_allocations = {
            let sender_type_for_log = sender_type;
            map_watcher(self.indexer_allocations.clone(), move |alloc_map| {
                let total = alloc_map.len();
                let mut legacy_count = 0usize;
                let mut horizon_count = 0usize;
                let set: HashSet<CollectionId> = alloc_map
                    .iter()
                    .filter_map(|(addr, alloc)| {
                        if alloc.is_legacy {
                            legacy_count += 1;
                            None
                        } else {
                            horizon_count += 1;
                            Some(CollectionId::from(*addr))
                        }
                    })
                    .collect();

                tracing::info!(
                    ?sender_type_for_log,
                    total,
                    legacy = legacy_count,
                    horizon = horizon_count,
                    normalized = set.len(),
                    "Normalized indexer allocations using isLegacy"
                );
                set
            })
        };

        Ok(SenderAccountArgs {
            config: self.config,
            pgpool: self.pgpool.clone(),
            sender_id: *sender_id,
            escrow_accounts,
            indexer_allocations,
            network_subgraph: self.network_subgraph,
            domain_separator_v2: self.domain_separator_v2.clone(),
            sender_aggregator_endpoint: self
                .sender_aggregator_endpoints
                .get(sender_id)
                .ok_or(anyhow!(
                    "No sender_aggregator_endpoints found for sender {}",
                    sender_id
                ))?
                .clone(),
            allocation_ids,
            prefix: self.prefix.clone(),
            retry_interval: RETRY_INTERVAL,
            sender_type,
        })
    }
}

/// Continuously listens for receipt notifications from a tokio channel and forwards them to the
/// corresponding SenderAccount.
async fn channel_receipts_watcher(
    actor_cell: ActorCell,
    mut rx: tokio::sync::mpsc::Receiver<ChannelReceiptNotification>,
    escrow_accounts_v2: Receiver<EscrowAccounts>,
    prefix: Option<String>,
) {
    tracing::info!(
        "Channel receipts watcher started (unified binary mode), prefix: {:?}",
        prefix
    );

    while let Some(notification) = rx.recv().await {
        let ChannelReceiptNotification {
            notification: receipt_notification,
        } = notification;

        tracing::debug!(
            receipt_id = receipt_notification.id(),
            value = receipt_notification.value(),
            "Received receipt notification from channel"
        );

        match handle_notification(
            receipt_notification,
            escrow_accounts_v2.clone(),
            prefix.as_deref(),
        )
        .await
        {
            Ok(()) => {
                tracing::debug!(
                    event = "channel_notification_handled",
                    "Successfully handled channel notification"
                );
            }
            Err(e) => {
                tracing::error!(error = %e, "Error handling channel notification");
            }
        }
    }

    // Channel closed - this is expected during shutdown
    tracing::info!("Channel receipts watcher shutting down - channel closed");

    // Only kill the manager if this is unexpected (not during shutdown)
    // In the unified binary, the service controls shutdown, so we just exit gracefully
    if actor_cell.get_status() == ractor::ActorStatus::Running {
        tracing::warn!("Channel closed unexpectedly while manager is still running");
        actor_cell
            .kill_and_wait(None)
            .await
            .expect("Failed to kill manager.");
    }
}

/// Handles a new detected [NewReceiptNotification] and routes to proper
/// reference of [super::sender_allocation::SenderAllocation]
///
/// If the allocation doesn't exist yet, we trust that the whoever has
/// access to the database already verified that the allocation really
/// exists and we ask for the sender to create a new allocation.
///
/// After a request to create allocation, we don't need to do anything
/// since the startup script is going to recalculate the receipt in the
/// database
#[tracing::instrument(
    skip_all,
    fields(
        sender_address = %new_receipt_notification.signer_address(),
        collection_id = %new_receipt_notification.collection_id(),
    )
)]
async fn handle_notification(
    new_receipt_notification: NewReceiptNotification,
    escrow_accounts_rx: Receiver<EscrowAccounts>,
    prefix: Option<&str>,
) -> anyhow::Result<()> {
    tracing::trace!(
        notification = ?new_receipt_notification,
        "New receipt notification detected!"
    );
    let escrow_accounts = escrow_accounts_rx.borrow();
    let sender_type_str = "V2";

    let signer = new_receipt_notification.signer_address();
    tracing::debug!(
        sender_type_str,
        signer = ?signer,
        "Looking up sender for signer in escrow accounts",
    );

    let Ok(sender_address) = escrow_accounts.get_sender_for_signer(&signer) else {
        tracing::error!(
            signer=?signer,
            sender_type_str,
            "ESCROW LOOKUP FAILURE: No sender found for signer in escrow accounts",
        );

        // TODO: save the receipt in the failed receipts table?
        bail!(
            "No sender address found for receipt signer address {} in {} escrow accounts. \
                    This suggests either: (1) escrow accounts not yet loaded or (2) signer not authorized.",
            signer,
            sender_type_str,
        );
    };

    let collection_id = new_receipt_notification.collection_id();
    let allocation_str = collection_id_hex(&collection_id);
    tracing::info!(
        sender_address = %sender_address,
        collection_id = %allocation_str,
        sender_type = sender_type_str,
        receipt_value = %new_receipt_notification.value(),
        "Processing receipt notification",
    );

    // For actor lookup, use the address format that matches how actors are created
    // "0x...."
    let allocation_for_actor_name = collection_id.as_address().to_string();

    let actor_name = format!(
        "{}{sender_address}:{allocation_for_actor_name}",
        prefix
            .as_ref()
            .map_or(String::default(), |prefix| format!("{prefix}:"))
    );

    // this logs must match regarding allocation type with
    // logs in   sender_account.rs:1174
    // otherwise there is a mistmatch!!!!
    tracing::debug!(
        actor_name,
        allocation_id = %collection_id,
        "Looking for SenderAllocation actor",
    );

    let Some(sender_allocation) = ActorRef::<SenderAllocationMessage>::where_is(actor_name) else {
        tracing::warn!(
            sender_address=%sender_address,
            allocation_id=%collection_id,
            "No sender_allocation found for sender_address and allocation_id to process new \
                receipt notification. Starting a new sender_allocation.",
        );

        let type_segment = "horizon:";

        let sender_account_name = format!(
            "{}{}{sender_address}",
            prefix
                .as_ref()
                .map_or(String::default(), |prefix| format!("{prefix}:")),
            type_segment,
        );
        tracing::debug!(
            sender_account_name,
            allocation_id = %collection_id,
            "Looking for SenderAccount",
        );

        let Some(sender_account) = ActorRef::<SenderAccountMessage>::where_is(sender_account_name)
        else {
            bail!(
                "No sender_account was found for address: {}.",
                sender_address
            );
        };
        sender_account
            .cast(SenderAccountMessage::NewAllocationId(collection_id))
            .map_err(|e| {
                anyhow!(
                    "Error while sendeing new allocation id message to sender_account: {:?}",
                    e
                )
            })?;
        return Ok(());
    };
    sender_allocation
        .cast(SenderAllocationMessage::NewReceipt(
            new_receipt_notification,
        ))
        .map_err(|e| {
            anyhow::anyhow!(
                "Error while forwarding new receipt notification to sender_allocation: {:?}",
                e
            )
        })?;
    RECEIPTS_CREATED
        .with_label_values(&[&sender_address.to_string(), &allocation_str])
        .inc();
    Ok(())
}

/// Force initialization of all LazyLock metrics in this module.
///
/// This ensures metrics are registered with Prometheus at startup,
/// even if no receipts have been processed yet.
pub fn init_metrics() {
    // Dereference each LazyLock to force initialization
    let _ = &*RECEIPTS_CREATED;
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use indexer_monitor::{DeploymentDetails, EscrowAccounts, SubgraphClient};
    use ractor::{Actor, ActorRef, ActorStatus};
    use reqwest::Url;
    use ruint::aliases::U256;
    use sqlx::PgPool;
    use test_assets::{
        assert_while_retry, flush_messages, TAP_SENDER as SENDER, TAP_SIGNER as SIGNER,
    };
    use thegraph_core::{alloy::hex::ToHexExt, CollectionId};
    use tokio::sync::{
        mpsc::{self, error::TryRecvError},
        watch,
    };

    use super::{
        channel_receipts_watcher, ChannelReceiptNotification, NewReceiptNotification,
        SenderAccountsManagerMessage, State,
    };
    use crate::{
        agent::{
            sender_account::SenderAccountMessage,
            sender_accounts_manager::{handle_notification, SenderType},
        },
        tap::TapReceipt,
        test::{
            actors::{DummyActor, MockSenderAccount, MockSenderAllocation, TestableActor},
            create_rav_v2, create_received_receipt, create_sender_accounts_manager,
            generate_random_prefix, get_grpc_url, get_sender_account_config, store_rav_v2,
            store_receipt, ALLOCATION_ID_0, ALLOCATION_ID_1, INDEXER, SENDER_2,
            TAP_EIP712_DOMAIN_SEPARATOR_V2,
        },
    };
    const DUMMY_URL: &str = "http://localhost:1234";

    async fn get_subgraph_client() -> &'static SubgraphClient {
        Box::leak(Box::new(
            SubgraphClient::new(
                reqwest::Client::new(),
                None,
                DeploymentDetails::for_query_url(DUMMY_URL).unwrap(),
            )
            .await,
        ))
    }

    struct TestState {
        prefix: String,
        state: State,
        _test_db: test_assets::TestDatabase,
    }

    async fn setup_state() -> TestState {
        let test_db = test_assets::setup_shared_test_db().await;
        let (prefix, state) = create_state(test_db.pool.clone()).await;
        TestState {
            prefix,
            state,
            _test_db: test_db,
        }
    }
    async fn setup_supervisor() -> ActorRef<()> {
        DummyActor::spawn().await
    }

    #[tokio::test]
    async fn test_create_sender_accounts_manager() {
        let test_db = test_assets::setup_shared_test_db().await;
        let pgpool = test_db.pool;
        let (_, _, (actor, join_handle), _notification_tx) =
            create_sender_accounts_manager().pgpool(pgpool).call().await;
        actor.stop_and_wait(None, None).await.unwrap();
        join_handle.await.unwrap();
    }

    async fn create_state(pgpool: PgPool) -> (String, State) {
        let config = get_sender_account_config();
        let senders_to_signers = vec![(SENDER.1, vec![SIGNER.1])].into_iter().collect();
        let escrow_accounts = EscrowAccounts::new(HashMap::new(), senders_to_signers);

        let prefix = generate_random_prefix();
        (
            prefix.clone(),
            State {
                config,
                domain_separator_v2: TAP_EIP712_DOMAIN_SEPARATOR_V2.clone(),
                sender_ids_v2: HashSet::new(),
                channel_receipts_watcher_handle: None,
                pgpool,
                indexer_allocations: watch::channel(HashMap::new()).1,
                escrow_accounts_v2: watch::channel(escrow_accounts).1,
                network_subgraph: get_subgraph_client().await,
                sender_aggregator_endpoints: HashMap::from([
                    (SENDER.1, Url::parse(&get_grpc_url().await).unwrap()),
                    (SENDER_2.1, Url::parse(&get_grpc_url().await).unwrap()),
                ]),
                prefix: Some(prefix),
            },
        )
    }

    #[tokio::test]
    async fn test_pending_sender_allocations() {
        let test_db = test_assets::setup_shared_test_db().await;
        let pgpool = test_db.pool;
        let (_, state) = create_state(pgpool.clone()).await;
        // add receipts to the database (stored in tap_horizon_receipts)
        for i in 1..=10 {
            let receipt = create_received_receipt(&ALLOCATION_ID_0, &SIGNER.0, i, i, i.into());
            store_receipt(&pgpool, receipt.signed_receipt())
                .await
                .unwrap();
        }
        // add non-final ravs (stored in tap_horizon_ravs)
        let collection_id_1 = *CollectionId::from(ALLOCATION_ID_1);
        let signed_rav = create_rav_v2(collection_id_1, SIGNER.0.clone(), 4, 10);
        store_rav_v2(&pgpool, signed_rav, SENDER.1).await.unwrap();

        let pending_allocation_id = state.get_pending_sender_allocation_id_v2().await;

        // check if pending allocations are correct
        assert_eq!(pending_allocation_id.len(), 1);
        assert!(pending_allocation_id.contains_key(&SENDER.1));
        assert_eq!(pending_allocation_id.get(&SENDER.1).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_update_sender_account() {
        let test_db = test_assets::setup_shared_test_db().await;
        let pgpool = test_db.pool;
        let (prefix, mut notify, (actor, join_handle), _notification_tx) =
            create_sender_accounts_manager().pgpool(pgpool).call().await;

        actor
            .cast(SenderAccountsManagerMessage::UpdateSenderAccountsV2(
                vec![SENDER.1].into_iter().collect(),
            ))
            .unwrap();

        flush_messages(&mut notify).await;

        assert_while_retry! {
            ActorRef::<SenderAccountMessage>::where_is(format!(
                "{}:horizon:{}",
                prefix.clone(),
                SENDER.1
            )).is_none()
        };

        // verify if create sender account
        let sender_ref = ActorRef::<SenderAccountMessage>::where_is(format!(
            "{}:horizon:{}",
            prefix.clone(),
            SENDER.1
        ))
        .unwrap();

        actor
            .cast(SenderAccountsManagerMessage::UpdateSenderAccountsV2(
                HashSet::new(),
            ))
            .unwrap();

        flush_messages(&mut notify).await;

        sender_ref.wait(None).await.unwrap();
        // verify if it gets removed
        let actor_ref =
            ActorRef::<SenderAccountMessage>::where_is(format!("{}:{}", prefix, SENDER.1));
        assert!(actor_ref.is_none());

        // safely stop the manager
        actor.stop_and_wait(None, None).await.unwrap();
        join_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_create_sender_account() {
        let state = setup_state().await;
        let supervisor = setup_supervisor().await;
        // we wait to check if the sender is created
        state
            .state
            .create_sender_account(
                supervisor.get_cell(),
                SENDER_2.1,
                HashSet::new(),
                SenderType::Horizon,
            )
            .await
            .unwrap();

        let actor_ref = ActorRef::<SenderAccountMessage>::where_is(format!(
            "{}:horizon:{}",
            state.prefix, SENDER_2.1
        ));
        assert!(actor_ref.is_some());
    }

    #[tokio::test]
    async fn test_deny_sender_account_on_failure() {
        let test_db = test_assets::setup_shared_test_db().await;
        let pgpool = test_db.pool;
        let supervisor = DummyActor::spawn().await;
        let (_prefix, state) = create_state(pgpool.clone()).await;
        state
            .create_or_deny_sender(
                supervisor.get_cell(),
                INDEXER.1,
                HashSet::new(),
                SenderType::Horizon,
            )
            .await;

        let denied = sqlx::query!(
            r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM tap_horizon_denylist
                    WHERE sender_address = $1
                ) as denied
            "#,
            INDEXER.1.encode_hex(),
        )
        .fetch_one(&pgpool)
        .await
        .unwrap()
        .denied
        .expect("Deny status cannot be null");

        assert!(denied, "Sender was not denied after failing.");
    }

    #[tokio::test]
    async fn test_receive_notifications() {
        let test_db = test_assets::setup_shared_test_db().await;
        let pgpool = test_db.pool;
        let prefix = generate_random_prefix();
        // create dummy allocation

        let (mock_sender_allocation, mut receipts) = MockSenderAllocation::new_with_receipts();
        let (tx, mut notify) = mpsc::channel(10);
        let actor = TestableActor::new(mock_sender_allocation, tx);
        let _ = Actor::spawn(
            Some(format!(
                "{}:{}:{}",
                prefix.clone(),
                SENDER.1,
                ALLOCATION_ID_0
            )),
            actor,
            (),
        )
        .await
        .unwrap();

        let (notification_tx, notification_rx) = mpsc::channel(10);

        let escrow_accounts_rx = watch::channel(EscrowAccounts::new(
            HashMap::from([(SENDER.1, U256::from(1000))]),
            HashMap::from([(SENDER.1, vec![SIGNER.1])]),
        ))
        .1;
        let dummy_actor = DummyActor::spawn().await;

        // Start the channel receipts watcher task
        let channel_receipts_watcher_handle = tokio::spawn(channel_receipts_watcher(
            dummy_actor.get_cell(),
            notification_rx,
            escrow_accounts_rx,
            Some(prefix.clone()),
        ));

        let receipts_count = 10;
        // add receipts to the database
        for i in 1..=receipts_count {
            let receipt = create_received_receipt(&ALLOCATION_ID_0, &SIGNER.0, i, i, i.into());
            let receipt_id = store_receipt(&pgpool, receipt.signed_receipt())
                .await
                .unwrap();
            let signed_receipt = receipt.signed_receipt();
            let TapReceipt::V2(signed_receipt) = signed_receipt;
            notification_tx
                .send(ChannelReceiptNotification {
                    notification: NewReceiptNotification {
                        id: receipt_id,
                        collection_id: signed_receipt.message.collection_id.encode_hex(),
                        signer_address: SIGNER.1,
                        timestamp_ns: signed_receipt.message.timestamp_ns,
                        value: signed_receipt.message.value,
                    },
                })
                .await
                .unwrap();
        }
        flush_messages(&mut notify).await;

        // check if receipt notification was sent to the allocation
        for i in 1..=receipts_count {
            let receipt = receipts.recv().await.unwrap();

            assert_eq!(i, receipt.id());
        }
        assert_eq!(receipts.try_recv().unwrap_err(), TryRecvError::Empty);

        channel_receipts_watcher_handle.abort();
    }

    #[tokio::test]
    async fn test_manager_killed_when_channel_closed() {
        let (notification_tx, notification_rx) = mpsc::channel(1);
        let escrow_accounts_rx = watch::channel(EscrowAccounts::default()).1;
        let dummy_actor = DummyActor::spawn().await;

        let channel_receipts_watcher_handle = tokio::spawn(channel_receipts_watcher(
            dummy_actor.get_cell(),
            notification_rx,
            escrow_accounts_rx,
            None,
        ));

        drop(notification_tx);
        channel_receipts_watcher_handle.await.unwrap();

        assert_eq!(dummy_actor.get_status(), ActorStatus::Stopped)
    }

    #[tokio::test]
    async fn test_create_collection_id() {
        let senders_to_signers = vec![(SENDER.1, vec![SIGNER.1])].into_iter().collect();
        let escrow_accounts = EscrowAccounts::new(HashMap::new(), senders_to_signers);
        let escrow_accounts = watch::channel(escrow_accounts).1;

        let prefix = generate_random_prefix();

        let (last_message_emitted, mut rx) = mpsc::channel(64);

        let (sender_account, join_handle) = MockSenderAccount::spawn(
            Some(format!("{}:horizon:{}", prefix.clone(), SENDER.1,)),
            MockSenderAccount {
                last_message_emitted,
            },
            (),
        )
        .await
        .unwrap();

        let new_receipt_notification = NewReceiptNotification {
            id: 1,
            collection_id: CollectionId::from(ALLOCATION_ID_0).encode_hex(),
            signer_address: SIGNER.1,
            timestamp_ns: 1,
            value: 1,
        };

        handle_notification(new_receipt_notification, escrow_accounts, Some(&prefix))
            .await
            .unwrap();

        let new_alloc_msg = rx.recv().await.unwrap();
        insta::assert_debug_snapshot!(new_alloc_msg);
        sender_account.stop_and_wait(None, None).await.unwrap();
        join_handle.await.unwrap();
    }
}
