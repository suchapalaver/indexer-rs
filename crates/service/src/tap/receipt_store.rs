// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use anyhow::anyhow;
use bigdecimal::num_bigint::BigInt;
use indexer_tap_agent::agent::sender_accounts_manager::ChannelReceiptNotification;
use sqlx::{types::BigDecimal, PgPool};
use tap_core::{manager::adapters::ReceiptStore, receipt::WithValueAndTimestamp};
use thegraph_core::alloy::{hex::ToHexExt, sol_types::Eip712Domain};
use tokio::{
    sync::{mpsc::Receiver, oneshot::Sender as OneShotSender},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{AdapterError, CheckingReceipt, IndexerTapContext, TapReceipt};

#[derive(Clone)]
pub struct InnerContext {
    pub pgpool: PgPool,
    /// Optional channel to send receipt notifications to TAP agent (unified binary mode).
    ///
    /// When set, receipt notifications will be sent through this channel after storage,
    /// enabling faster processing than pg_notify. Currently unused pending implementation
    /// of receipt ID retrieval from bulk INSERT (pg_notify will be used as fallback).
    #[allow(dead_code)]
    pub tap_agent_tx: Option<tokio::sync::mpsc::Sender<ChannelReceiptNotification>>,
}

#[derive(thiserror::Error, Debug)]
enum ProcessReceiptError {
    #[error("Failed to store v2 receipts: {0}")]
    V2(anyhow::Error),
}

/// Indicates which versions of Receipts were processed
/// It's intended to be used for migration tests
#[derive(Debug, PartialEq, Eq)]
pub enum ProcessedReceipt {
    V2,
    None,
}

impl InnerContext {
    async fn process_db_receipts(
        &self,
        buffer: Vec<(DatabaseReceipt, OneShotSender<Result<(), AdapterError>>)>,
    ) -> Result<ProcessedReceipt, ProcessReceiptError> {
        // V2 (Horizon) only - V1/Legacy support has been removed
        let (v2_receipts, v2_senders): (Vec<_>, Vec<_>) = buffer
            .into_iter()
            .map(|(receipt, sender)| {
                let DatabaseReceipt::V2(r) = receipt;
                (r, sender)
            })
            .unzip();

        let insert_v2 = self.store_receipts_v2(v2_receipts).await;

        // send back the result of storing receipts to callers
        Self::notify_senders(v2_senders, &insert_v2, "V2");

        match insert_v2 {
            Err(e2) => Err(ProcessReceiptError::V2(e2.into())),
            Ok(None) => Ok(ProcessedReceipt::None),
            Ok(Some(_)) => Ok(ProcessedReceipt::V2),
        }
    }

    fn notify_senders(
        senders: Vec<OneShotSender<Result<(), AdapterError>>>,
        result: &Result<Option<u64>, AdapterError>,
        version: &str,
    ) {
        match result {
            Ok(_) => {
                for sender in senders {
                    let _ = sender.send(Ok(()));
                }
            }
            Err(e) => {
                // Create error message once
                let err_msg = format!("Failed to store {version} receipts: {e}");
                tracing::error!(error = %e, version = %version, "Failed to store receipts");
                for sender in senders {
                    // Convert to AdapterError for each sender
                    let _ = sender.send(Err(anyhow!(err_msg.clone()).into()));
                }
            }
        }
    }

    async fn store_receipts_v2(
        &self,
        receipts: Vec<DbReceiptV2>,
    ) -> Result<Option<u64>, AdapterError> {
        if receipts.is_empty() {
            return Ok(None);
        }
        let receipts_len = receipts.len();
        let mut signers = Vec::with_capacity(receipts_len);
        let mut signatures = Vec::with_capacity(receipts_len);
        let mut collection_ids = Vec::with_capacity(receipts_len);
        let mut payers = Vec::with_capacity(receipts_len);
        let mut data_services = Vec::with_capacity(receipts_len);
        let mut service_providers = Vec::with_capacity(receipts_len);
        let mut timestamps = Vec::with_capacity(receipts_len);
        let mut nonces = Vec::with_capacity(receipts_len);
        let mut values = Vec::with_capacity(receipts_len);

        for receipt in receipts {
            signers.push(receipt.signer_address);
            signatures.push(receipt.signature);
            collection_ids.push(receipt.collection_id);
            payers.push(receipt.payer);
            data_services.push(receipt.data_service);
            service_providers.push(receipt.service_provider);
            timestamps.push(receipt.timestamp_ns);
            nonces.push(receipt.nonce);
            values.push(receipt.value);
        }
        let query_res = sqlx::query!(
            r#"INSERT INTO tap_horizon_receipts (
                signer_address,
                signature,
                collection_id,
                payer,
                data_service,
                service_provider,
                timestamp_ns,
                nonce,
                value
            ) SELECT * FROM UNNEST(
                $1::CHAR(40)[],
                $2::BYTEA[],
                $3::CHAR(64)[],
                $4::CHAR(40)[],
                $5::CHAR(40)[],
                $6::CHAR(40)[],
                $7::NUMERIC(20)[],
                $8::NUMERIC(20)[],
                $9::NUMERIC(40)[]
            )"#,
            &signers,
            &signatures,
            &collection_ids,
            &payers,
            &data_services,
            &service_providers,
            &timestamps,
            &nonces,
            &values,
        )
        .execute(&self.pgpool)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to store V2 receipt");
            anyhow!(e)
        })?;

        Ok(Some(query_res.rows_affected()))
    }
}

impl IndexerTapContext {
    pub fn spawn_store_receipt_task(
        inner_context: InnerContext,
        mut receiver: Receiver<(DatabaseReceipt, OneShotSender<Result<(), AdapterError>>)>,
        cancelation_token: CancellationToken,
    ) -> JoinHandle<()> {
        const BUFFER_SIZE: usize = 100;
        tokio::spawn(async move {
            loop {
                let mut buffer = Vec::with_capacity(BUFFER_SIZE);
                tokio::select! {
                    biased;
                    _ = receiver.recv_many(&mut buffer, BUFFER_SIZE) => {
                        if let Err(e) = inner_context.process_db_receipts(buffer).await {
                            tracing::error!(error = %e, "Failed to process buffered receipts");
                        }
                    }
                    _ = cancelation_token.cancelled() => { break },
                }
            }
        })
    }
}

#[async_trait::async_trait]
impl ReceiptStore<TapReceipt> for IndexerTapContext {
    type AdapterError = AdapterError;

    async fn store_receipt(&self, receipt: CheckingReceipt) -> Result<u64, Self::AdapterError> {
        // V2 (Horizon) only - V1/Legacy support has been removed
        let TapReceipt::V2(_) = receipt.signed_receipt();
        let separator = &self.domain_separator_v2;
        let db_receipt = DatabaseReceipt::from_receipt(receipt, separator)?;
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.receipt_producer
            .send((db_receipt, result_tx))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to queue receipt for storage");
                anyhow!(e)
            })?;

        let res = result_rx.await.map_err(|e| anyhow!(e))?;

        // We don't need receipt_ids
        res.map(|_| 0)
    }
}

/// Database representation of a V2 receipt (V1/Legacy support has been removed)
pub enum DatabaseReceipt {
    V2(DbReceiptV2),
}

impl DatabaseReceipt {
    fn from_receipt(receipt: CheckingReceipt, separator: &Eip712Domain) -> anyhow::Result<Self> {
        let TapReceipt::V2(receipt) = receipt.signed_receipt();
        Ok(Self::V2(DbReceiptV2::from_receipt(receipt, separator)?))
    }
}

pub struct DbReceiptV2 {
    signer_address: String,
    signature: Vec<u8>,
    collection_id: String,
    payer: String,
    data_service: String,
    service_provider: String,
    timestamp_ns: BigDecimal,
    nonce: BigDecimal,
    value: BigDecimal,
}

impl DbReceiptV2 {
    fn from_receipt(
        receipt: &tap_graph::v2::SignedReceipt,
        separator: &Eip712Domain,
    ) -> anyhow::Result<Self> {
        let collection_id =
            thegraph_core::CollectionId::from(receipt.message.collection_id).encode_hex();

        let payer = receipt.message.payer.encode_hex();
        let data_service = receipt.message.data_service.encode_hex();
        let service_provider = receipt.message.service_provider.encode_hex();
        let signature = receipt.signature.as_bytes().to_vec();

        let signer_address = receipt
            .recover_signer(separator)
            .map_err(|e| {
                tracing::error!(error = %e, "Failed to recover V2 receipt signer");
                anyhow!(e)
            })?
            .encode_hex();

        let timestamp_ns = BigDecimal::from(receipt.timestamp_ns());
        let nonce = BigDecimal::from(receipt.message.nonce);
        let value = BigDecimal::from(BigInt::from(receipt.value()));
        Ok(Self {
            collection_id,
            payer,
            data_service,
            service_provider,
            nonce,
            signature,
            signer_address,
            timestamp_ns,
            value,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures::future::BoxFuture;
    use sqlx::migrate::{MigrationSource, Migrator};
    use test_assets::{create_signed_receipt_v2, TAP_EIP712_DOMAIN_V2};

    use crate::tap::{
        receipt_store::{
            DatabaseReceipt, DbReceiptV2, InnerContext, ProcessReceiptError, ProcessedReceipt,
        },
        AdapterError,
    };

    async fn create_v2() -> DatabaseReceipt {
        let v2 = create_signed_receipt_v2().call().await;
        DatabaseReceipt::V2(DbReceiptV2::from_receipt(&v2, &TAP_EIP712_DOMAIN_V2).unwrap())
    }

    pub type VecReceiptTx = Vec<(
        DatabaseReceipt,
        tokio::sync::oneshot::Sender<Result<(), AdapterError>>,
    )>;
    pub type VecRx = Vec<tokio::sync::oneshot::Receiver<Result<(), AdapterError>>>;

    pub fn attach_oneshot_channels(receipts: Vec<DatabaseReceipt>) -> (VecReceiptTx, VecRx) {
        let mut txs = Vec::with_capacity(receipts.len());
        let mut rxs = Vec::with_capacity(receipts.len());
        for r in receipts.into_iter() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            txs.push((r, tx));
            rxs.push(rx);
        }
        (txs, rxs)
    }

    mod when_all_migrations_are_run {
        use super::*;

        #[rstest::rstest]
        #[case(ProcessedReceipt::None, async { vec![] })]
        #[case(ProcessedReceipt::V2, async { vec![create_v2().await] })]
        #[tokio::test]
        async fn v2_receipts_are_processed_successfully(
            #[case] expected: ProcessedReceipt,
            #[future(awt)]
            #[case]
            receipts: Vec<DatabaseReceipt>,
        ) {
            let test_db = test_assets::setup_shared_test_db().await;
            let context = InnerContext {
                pgpool: test_db.pool,
                tap_agent_tx: None,
            };
            let (receipts, _rxs) = attach_oneshot_channels(receipts);

            let res = context.process_db_receipts(receipts).await.unwrap();

            assert_eq!(res, expected);
        }
    }

    mod when_horizon_migrations_are_ignored {
        use super::*;

        #[tokio::test]
        async fn test_empty_receipts_are_processed_successfully() {
            let migrator = create_migrator();
            let test_db = test_assets::setup_test_db_with_migrator(migrator).await;
            let context = InnerContext {
                pgpool: test_db.pool,
                tap_agent_tx: None,
            };

            let res = context.process_db_receipts(vec![]).await.unwrap();

            assert_eq!(res, ProcessedReceipt::None);
        }

        #[tokio::test]
        async fn test_v2_receipts_fails_to_process_without_horizon_table() {
            // Create a database without horizon migrations by running a custom migrator
            // that excludes horizon-related migrations
            let migrator = create_migrator();
            let test_db = test_assets::setup_test_db_with_migrator(migrator).await;

            let context = InnerContext {
                pgpool: test_db.pool,
                tap_agent_tx: None,
            };

            let receipts = vec![create_v2().await];
            let (receipts, _rxs) = attach_oneshot_channels(receipts);
            let error = context.process_db_receipts(receipts).await.unwrap_err();

            let ProcessReceiptError::V2(error) = error;
            let d = error.downcast_ref::<AdapterError>().unwrap().to_string();

            assert_eq!(
                d,
                "error returned from database: relation \"tap_horizon_receipts\" does not exist"
            );
        }

        pub fn create_migrator() -> Migrator {
            futures::executor::block_on(Migrator::new(MigrationRunner::new(
                "../../migrations",
                ["horizon"],
            )))
            .unwrap()
        }

        #[derive(Debug)]
        pub struct MigrationRunner {
            migration_path: PathBuf,
            ignored_migrations: Vec<String>,
        }

        impl MigrationRunner {
            /// Construct a new MigrationRunner that does not apply the given migrations.
            ///
            /// `ignored_migrations` is any iterable of strings that describes which
            /// migrations to be ignored.
            pub fn new<I>(path: impl Into<PathBuf>, ignored_migrations: I) -> Self
            where
                I: IntoIterator,
                I::Item: Into<String>,
            {
                Self {
                    migration_path: path.into(),
                    ignored_migrations: ignored_migrations.into_iter().map(Into::into).collect(),
                }
            }
        }

        impl MigrationSource<'static> for MigrationRunner {
            fn resolve(
                self,
            ) -> BoxFuture<'static, Result<Vec<sqlx::migrate::Migration>, sqlx::error::BoxDynError>>
            {
                Box::pin(async move {
                    let canonical = self.migration_path.canonicalize()?;
                    let migrations_with_paths =
                        sqlx::migrate::resolve_blocking(&canonical).unwrap();

                    let migrations_with_paths = migrations_with_paths
                        .into_iter()
                        .filter(|(_, p)| {
                            let path = p.to_str().unwrap();
                            self.ignored_migrations
                                .iter()
                                .any(|ignored| !path.contains(ignored))
                        })
                        .collect::<Vec<_>>();

                    Ok(migrations_with_paths.into_iter().map(|(m, _p)| m).collect())
                })
            }
        }
    }
}
