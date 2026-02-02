// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use anyhow::anyhow;
use indexer_monitor::SubgraphClient;
use indexer_query::graph_tally_tokens_collected::{
    self as graph_tally_tokens_collected, GraphTallyTokensCollectedQuery,
};
use indexer_watcher::new_watcher;
use tap_core::receipt::checks::{Check, CheckError, CheckResult};
use thegraph_core::{alloy::primitives::Address, CollectionId};
use tokio::sync::watch::{self, Receiver};

use crate::tap::{CheckingReceipt, TapReceipt};

/// AllocationId check
///
/// Verifies if the allocation is already redeemed.
pub struct AllocationId {
    tap_allocation_redeemed: Receiver<bool>,
    allocation_id: Address,
}

impl AllocationId {
    /// Creates a new allocation id check
    pub async fn new(
        indexer_address: Address,
        escrow_polling_interval: Duration,
        sender_id: Address,
        collection_id: CollectionId,
        network_subgraph: &'static SubgraphClient,
    ) -> Self {
        let tap_allocation_redeemed = match tap_allocation_redeemed_watcher(
            collection_id,
            sender_id,
            indexer_address,
            network_subgraph,
            escrow_polling_interval,
        )
        .await
        {
            Ok(rx) => rx,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    sender = %sender_id,
                    indexer = %indexer_address,
                    "Failed to initialize tap_allocation_redeemed_watcher; assuming not redeemed"
                );
                let (_tx, rx) = watch::channel(false);
                rx
            }
        };

        Self {
            tap_allocation_redeemed,
            allocation_id: collection_id.as_address(),
        }
    }
}

#[async_trait::async_trait]
impl Check<TapReceipt> for AllocationId {
    async fn check(
        &self,
        _: &tap_core::receipt::Context,
        receipt: &CheckingReceipt,
    ) -> CheckResult {
        // V2 receipts provide collection_id which we map to an Address.
        // collection_id is 32 bytes with the 20-byte address right-aligned (left-padded zeros).
        let cid = receipt.signed_receipt().collection_id();
        let bytes = cid.as_slice();
        let allocation_id = Address::from_slice(&bytes[12..32]);

        tracing::debug!(
            allocation_id = %allocation_id,
            expected_allocation_id = %self.allocation_id,
            "Checking allocation_id",
        );
        if allocation_id != self.allocation_id {
            return Err(CheckError::Failed(anyhow!("Receipt allocation_id different from expected: allocation_id: {:?}, expected_allocation_id: {}", allocation_id, self.allocation_id)));
        };

        // Check that the allocation ID is not redeemed yet for this consumer
        match *self.tap_allocation_redeemed.borrow() {
            false => Ok(()),
            true => Err(CheckError::Failed(anyhow!(
                "Allocation {:?} already redeemed",
                allocation_id
            ))),
        }
    }
}

async fn tap_allocation_redeemed_watcher(
    collection_id: CollectionId,
    sender_address: Address,
    indexer_address: Address,
    network_subgraph: &'static SubgraphClient,
    escrow_polling_interval: Duration,
) -> anyhow::Result<Receiver<bool>> {
    new_watcher(escrow_polling_interval, move || async move {
        query_collected_tokens(
            collection_id,
            sender_address,
            indexer_address,
            network_subgraph,
        )
        .await
    })
    .await
}

async fn query_collected_tokens(
    collection_id: CollectionId,
    sender_address: Address,
    indexer_address: Address,
    network_subgraph: &'static SubgraphClient,
) -> anyhow::Result<bool> {
    let response = network_subgraph
        .query::<GraphTallyTokensCollectedQuery, _>(graph_tally_tokens_collected::Variables {
            payer: format!("{sender_address:x?}"),
            receiver: format!("{indexer_address:x?}"),
            collection_ids: vec![format!("{collection_id:x?}")],
        })
        .await?;

    response
        .map(|data| {
            data.graph_tally_tokens_collecteds
                .iter()
                .any(|entry| entry.tokens.parse::<u128>().unwrap_or(0) > 0)
        })
        .map_err(|err| anyhow!(err))
}

#[cfg(test)]
mod tests {
    use indexer_monitor::{DeploymentDetails, SubgraphClient};
    use thegraph_core::CollectionId;
    use wiremock::{matchers::body_string_contains, Mock, MockServer, ResponseTemplate};

    use super::query_collected_tokens;

    #[tokio::test]
    async fn test_transaction_exists() {
        let mock_network_subgraph = MockServer::start().await;
        mock_network_subgraph
            .register(
                Mock::given(body_string_contains("graphTallyTokensCollecteds")).respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "data": {
                            "graphTallyTokensCollecteds": [
                                { "collectionId": "0x01", "tokens": "1" }
                            ]
                        }
                    })),
                ),
            )
            .await;

        let network_subgraph = Box::leak(Box::new(
            SubgraphClient::new(
                reqwest::Client::new(),
                None,
                DeploymentDetails::for_query_url(&mock_network_subgraph.uri()).unwrap(),
            )
            .await,
        ));

        let result = query_collected_tokens(
            CollectionId::from(thegraph_core::alloy::primitives::FixedBytes::<32>::ZERO),
            "0x21fed3c4340f67dbf2b78c670ebd1940668ca03e"
                .parse()
                .unwrap(),
            "0x54d7db28ce0d0e2e87764cd09298f9e4e913e567"
                .parse()
                .unwrap(),
            network_subgraph,
        );

        assert!(result.await.unwrap());
    }
}
