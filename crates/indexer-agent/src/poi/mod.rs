// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Proof of Indexing (POI) resolution for allocation operations.
//!
//! This module provides functionality to query graph-node for POI data
//! needed when closing allocations. POI is required to claim indexing
//! rewards on-chain.
//!
//! # Usage
//!
//! ```ignore
//! let poi_resolver = PoiResolver::new(graph_node_status_url);
//! let poi_result = poi_resolver.resolve_poi(&deployment_id, block_number).await?;
//! ```

use alloy::primitives::{BlockNumber, FixedBytes};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thegraph_core::DeploymentId;
use tracing::{debug, warn};
use url::Url;

use crate::error::{ErrorClass, ErrorClassification};

/// Result of a POI resolution request.
#[derive(Debug, Clone)]
pub struct PoiResult {
    /// The deployment ID
    pub deployment: String,
    /// The block number for the POI
    pub block_number: BlockNumber,
    /// The public POI hash (bytes32)
    pub public_poi: FixedBytes<32>,
}

/// Errors that can occur during POI resolution.
#[derive(Debug, thiserror::Error)]
pub enum PoiError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// GraphQL error response
    #[error("GraphQL error: {0}")]
    GraphQL(String),

    /// POI not found for deployment at block
    #[error("POI not found for deployment {deployment} at block {block_number}")]
    NotFound {
        deployment: String,
        block_number: BlockNumber,
    },

    /// Failed to parse POI response
    #[error("failed to parse POI response: {0}")]
    ParseError(String),

    /// Deployment is not synced to the requested block
    #[error(
        "deployment {deployment} is not synced to block {block_number} (latest: {latest_block})"
    )]
    NotSynced {
        deployment: String,
        block_number: BlockNumber,
        latest_block: BlockNumber,
    },
}

impl ErrorClassification for PoiError {
    fn class(&self) -> ErrorClass {
        ErrorClass::External
    }
}

/// GraphQL request for publicProofsOfIndexing
#[derive(Debug, Serialize)]
struct PublicPoiRequest {
    query: &'static str,
    variables: PublicPoiVariables,
}

#[derive(Debug, Serialize)]
struct PublicPoiVariables {
    requests: Vec<PublicPoiInput>,
}

#[derive(Debug, Serialize)]
struct PublicPoiInput {
    deployment: String,
    #[serde(rename = "blockNumber")]
    block_number: String,
}

/// GraphQL response for publicProofsOfIndexing
#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct PublicPoiData {
    #[serde(rename = "publicProofsOfIndexing")]
    public_proofs_of_indexing: Vec<PublicPoiResult>,
}

#[derive(Debug, Deserialize)]
struct PublicPoiResult {
    deployment: String,
    block: PartialBlock,
    #[serde(rename = "proofOfIndexing")]
    proof_of_indexing: String,
}

#[derive(Debug, Deserialize)]
struct PartialBlock {
    number: String,
    #[allow(dead_code)]
    hash: Option<String>,
}

/// GraphQL request for indexingStatuses
#[derive(Debug, Serialize)]
struct IndexingStatusRequest {
    query: &'static str,
    variables: IndexingStatusVariables,
}

#[derive(Debug, Serialize)]
struct IndexingStatusVariables {
    subgraphs: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IndexingStatusData {
    #[serde(rename = "indexingStatuses")]
    indexing_statuses: Vec<IndexingStatus>,
}

#[derive(Debug, Deserialize)]
struct IndexingStatus {
    subgraph: String,
    chains: Vec<ChainIndexingStatus>,
}

#[derive(Debug, Deserialize)]
struct ChainIndexingStatus {
    #[serde(rename = "latestBlock")]
    latest_block: Option<Block>,
}

#[derive(Debug, Deserialize)]
struct Block {
    number: String,
}

/// POI resolver that queries graph-node for proof of indexing data.
#[derive(Debug, Clone)]
pub struct PoiResolver {
    /// HTTP client for making requests
    client: Client,
    /// Graph-node status endpoint URL
    status_url: Url,
}

impl PoiResolver {
    /// Create a new POI resolver.
    ///
    /// # Arguments
    /// * `status_url` - The graph-node status endpoint URL
    pub fn new(status_url: Url) -> Self {
        Self {
            client: Client::new(),
            status_url,
        }
    }

    /// Get the latest synced block number for a deployment.
    ///
    /// This queries the indexingStatuses endpoint to find the latest block
    /// that has been indexed for the given deployment.
    pub async fn get_latest_block(
        &self,
        deployment: &DeploymentId,
    ) -> Result<BlockNumber, PoiError> {
        let deployment_str = deployment.to_string();

        let request = IndexingStatusRequest {
            query: r#"
                query IndexingStatuses($subgraphs: [String!]) {
                    indexingStatuses(subgraphs: $subgraphs) {
                        subgraph
                        chains {
                            latestBlock {
                                number
                            }
                        }
                    }
                }
            "#,
            variables: IndexingStatusVariables {
                subgraphs: vec![deployment_str.clone()],
            },
        };

        let response = self
            .client
            .post(self.status_url.clone())
            .json(&request)
            .send()
            .await?
            .json::<GraphQLResponse<IndexingStatusData>>()
            .await?;

        // Check for GraphQL errors
        if let Some(errors) = response.errors {
            let messages: Vec<_> = errors.iter().map(|e| e.message.as_str()).collect();
            return Err(PoiError::GraphQL(messages.join("; ")));
        }

        // Extract latest block from response
        let data = response
            .data
            .ok_or_else(|| PoiError::GraphQL("no data in response".to_string()))?;

        let status = data
            .indexing_statuses
            .into_iter()
            .find(|s| s.subgraph == deployment_str)
            .ok_or_else(|| PoiError::NotFound {
                deployment: deployment_str.clone(),
                block_number: BlockNumber::from(0u64),
            })?;

        // Get the latest block from the first chain (there's typically only one)
        let latest_block = status
            .chains
            .first()
            .and_then(|c| c.latest_block.as_ref())
            .ok_or_else(|| PoiError::NotFound {
                deployment: deployment_str.clone(),
                block_number: BlockNumber::from(0u64),
            })?;

        let block_number: BlockNumber = latest_block
            .number
            .parse()
            .map_err(|e| PoiError::ParseError(format!("invalid block number: {e}")))?;

        debug!(
            deployment = %deployment_str,
            block_number = block_number,
            "Got latest synced block for deployment"
        );

        Ok(block_number)
    }

    /// Resolve POI for a deployment at a specific block number.
    ///
    /// This queries graph-node's `publicProofsOfIndexing` endpoint to get
    /// the public POI for the given deployment at the specified block.
    ///
    /// # Arguments
    /// * `deployment` - The deployment ID
    /// * `block_number` - The block number to get POI for
    ///
    /// # Returns
    /// The POI result containing the public POI hash, or an error if POI
    /// cannot be resolved.
    pub async fn resolve_poi(
        &self,
        deployment: &DeploymentId,
        block_number: BlockNumber,
    ) -> Result<PoiResult, PoiError> {
        let deployment_str = deployment.to_string();

        let request = PublicPoiRequest {
            query: r#"
                query PublicProofsOfIndexing($requests: [PublicProofOfIndexingRequest!]!) {
                    publicProofsOfIndexing(requests: $requests) {
                        deployment
                        block {
                            number
                            hash
                        }
                        proofOfIndexing
                    }
                }
            "#,
            variables: PublicPoiVariables {
                requests: vec![PublicPoiInput {
                    deployment: deployment_str.clone(),
                    block_number: block_number.to_string(),
                }],
            },
        };

        debug!(
            deployment = %deployment_str,
            block_number = block_number,
            url = %self.status_url,
            "Resolving POI from graph-node"
        );

        let response = self
            .client
            .post(self.status_url.clone())
            .json(&request)
            .send()
            .await?
            .json::<GraphQLResponse<PublicPoiData>>()
            .await?;

        // Check for GraphQL errors
        if let Some(errors) = response.errors {
            let messages: Vec<_> = errors.iter().map(|e| e.message.as_str()).collect();
            return Err(PoiError::GraphQL(messages.join("; ")));
        }

        // Extract POI from response
        let data = response
            .data
            .ok_or_else(|| PoiError::GraphQL("no data in response".to_string()))?;

        let poi_result = data
            .public_proofs_of_indexing
            .into_iter()
            .find(|p| p.deployment == deployment_str)
            .ok_or_else(|| PoiError::NotFound {
                deployment: deployment_str.clone(),
                block_number,
            })?;

        // Parse the POI hash (should be a hex string starting with 0x)
        let public_poi: FixedBytes<32> = poi_result
            .proof_of_indexing
            .parse()
            .map_err(|e| PoiError::ParseError(format!("invalid POI hash: {e}")))?;

        // Parse the block number from response
        let response_block: BlockNumber = poi_result
            .block
            .number
            .parse()
            .map_err(|e| PoiError::ParseError(format!("invalid block number: {e}")))?;

        debug!(
            deployment = %deployment_str,
            block_number = response_block,
            public_poi = %public_poi,
            "Successfully resolved POI"
        );

        Ok(PoiResult {
            deployment: deployment_str,
            block_number: response_block,
            public_poi,
        })
    }

    /// Resolve POI for a deployment at its latest synced block.
    ///
    /// This first queries the deployment's sync status to find the latest
    /// block, then resolves the POI at that block.
    ///
    /// # Arguments
    /// * `deployment` - The deployment ID
    ///
    /// # Returns
    /// The POI result, or an error if POI cannot be resolved.
    pub async fn resolve_poi_at_latest(
        &self,
        deployment: &DeploymentId,
    ) -> Result<PoiResult, PoiError> {
        let latest_block = self.get_latest_block(deployment).await?;
        self.resolve_poi(deployment, latest_block).await
    }
}

/// Try to resolve POI for closing an allocation.
///
/// This function handles the full POI resolution flow:
/// 1. If `force` is true, returns None (use zero POI)
/// 2. Otherwise, attempts to resolve POI from graph-node
/// 3. Returns the POI result or an error
///
/// # Arguments
/// * `resolver` - The POI resolver
/// * `deployment` - The deployment ID
/// * `force` - Whether to force close without POI
///
/// # Returns
/// - `Ok(Some(PoiResult))` if POI was resolved successfully
/// - `Ok(None)` if force=true (use zero POI)
/// - `Err(PoiError)` if POI resolution failed and force=false
pub async fn resolve_poi_for_close(
    resolver: &PoiResolver,
    deployment: &DeploymentId,
    force: bool,
) -> Result<Option<PoiResult>, PoiError> {
    if force {
        warn!(
            deployment = %deployment,
            "Force closing allocation without POI - indexing rewards will be forfeited"
        );
        return Ok(None);
    }

    let poi_result = resolver.resolve_poi_at_latest(deployment).await?;
    Ok(Some(poi_result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poi_result_creation() {
        let poi = PoiResult {
            deployment: "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY".to_string(),
            block_number: BlockNumber::from(12_345_678u64),
            public_poi: FixedBytes::ZERO,
        };

        assert_eq!(poi.block_number, BlockNumber::from(12_345_678u64));
        assert_eq!(poi.public_poi, FixedBytes::ZERO);
    }

    #[test]
    fn test_poi_error_messages() {
        let err = PoiError::NotFound {
            deployment: "QmTest".to_string(),
            block_number: BlockNumber::from(100u64),
        };
        assert!(err.to_string().contains("QmTest"));
        assert!(err.to_string().contains("100"));

        let err = PoiError::NotSynced {
            deployment: "QmTest".to_string(),
            block_number: BlockNumber::from(200u64),
            latest_block: BlockNumber::from(100u64),
        };
        assert!(err.to_string().contains("not synced"));
        assert!(err.to_string().contains("200"));
        assert!(err.to_string().contains("100"));
    }

    #[test]
    fn test_graphql_request_serialization() {
        let request = PublicPoiRequest {
            query: "query { test }",
            variables: PublicPoiVariables {
                requests: vec![PublicPoiInput {
                    deployment: "QmTest".to_string(),
                    block_number: "12345".to_string(),
                }],
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("blockNumber"));
        assert!(json.contains("12345"));
    }
}
