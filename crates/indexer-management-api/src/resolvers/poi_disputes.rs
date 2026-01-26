// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::{Context, InputObject, Object, SimpleObject};
use indexer_agent::{POIDispute as AgentPOIDispute, POIDisputeInput as AgentPOIDisputeInput};
use sqlx::PgPool;

/// GraphQL output type for POIDispute
#[derive(Debug, Clone, SimpleObject)]
pub struct POIDispute {
    pub allocation_id: String,
    pub subgraph_deployment_id: String,
    pub allocation_indexer: String,
    pub allocation_amount: String,
    pub allocation_proof: String,
    pub closed_epoch: i32,
    pub closed_epoch_reference_proof: Option<String>,
    pub closed_epoch_start_block_hash: String,
    pub closed_epoch_start_block_number: i32,
    pub previous_epoch_reference_proof: Option<String>,
    pub previous_epoch_start_block_hash: String,
    pub previous_epoch_start_block_number: i32,
    pub status: String,
    pub protocol_network: String,
}

impl From<AgentPOIDispute> for POIDispute {
    fn from(dispute: AgentPOIDispute) -> Self {
        Self {
            allocation_id: dispute.allocation_id,
            subgraph_deployment_id: dispute.subgraph_deployment_id,
            allocation_indexer: dispute.allocation_indexer,
            allocation_amount: dispute.allocation_amount.to_string(),
            allocation_proof: dispute.allocation_proof,
            closed_epoch: dispute.closed_epoch,
            closed_epoch_reference_proof: dispute.closed_epoch_reference_proof,
            closed_epoch_start_block_hash: dispute.closed_epoch_start_block_hash,
            closed_epoch_start_block_number: dispute.closed_epoch_start_block_number,
            previous_epoch_reference_proof: dispute.previous_epoch_reference_proof,
            previous_epoch_start_block_hash: dispute.previous_epoch_start_block_hash,
            previous_epoch_start_block_number: dispute.previous_epoch_start_block_number,
            status: dispute.status,
            protocol_network: dispute.protocol_network,
        }
    }
}

/// GraphQL input type for creating POI disputes
#[derive(Debug, Clone, InputObject)]
pub struct POIDisputeInput {
    pub allocation_id: String,
    pub subgraph_deployment_id: String,
    pub allocation_indexer: String,
    pub allocation_amount: String,
    pub allocation_proof: String,
    pub closed_epoch: i32,
    pub closed_epoch_reference_proof: Option<String>,
    pub closed_epoch_start_block_hash: String,
    pub closed_epoch_start_block_number: i32,
    pub previous_epoch_reference_proof: Option<String>,
    pub previous_epoch_start_block_hash: String,
    pub previous_epoch_start_block_number: i32,
    pub status: String,
    pub protocol_network: String,
}

impl TryFrom<POIDisputeInput> for AgentPOIDisputeInput {
    type Error = anyhow::Error;

    fn try_from(input: POIDisputeInput) -> Result<Self, Self::Error> {
        Ok(Self {
            allocation_id: input.allocation_id,
            subgraph_deployment_id: input.subgraph_deployment_id,
            allocation_indexer: input.allocation_indexer,
            allocation_amount: input.allocation_amount.parse().map_err(|_| {
                anyhow::anyhow!("Invalid allocation amount: must be a valid decimal")
            })?,
            allocation_proof: input.allocation_proof,
            closed_epoch: input.closed_epoch,
            closed_epoch_reference_proof: input.closed_epoch_reference_proof,
            closed_epoch_start_block_hash: input.closed_epoch_start_block_hash,
            closed_epoch_start_block_number: input.closed_epoch_start_block_number,
            previous_epoch_reference_proof: input.previous_epoch_reference_proof,
            previous_epoch_start_block_hash: input.previous_epoch_start_block_hash,
            previous_epoch_start_block_number: input.previous_epoch_start_block_number,
            status: input.status,
            protocol_network: input.protocol_network,
        })
    }
}

#[derive(Default)]
pub struct POIDisputeQuery;

#[Object]
impl POIDisputeQuery {
    /// Get a single dispute by allocation ID
    async fn dispute(
        &self,
        ctx: &Context<'_>,
        allocation_id: String,
        protocol_network: String,
    ) -> Result<Option<POIDispute>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let dispute = AgentPOIDispute::get(pool, &allocation_id, &protocol_network).await?;
        Ok(dispute.map(Into::into))
    }

    /// Get all disputes for a protocol network
    async fn disputes(
        &self,
        ctx: &Context<'_>,
        protocol_network: String,
        status: Option<String>,
    ) -> Result<Vec<POIDispute>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let disputes = if let Some(status) = status {
            AgentPOIDispute::get_by_status(pool, &status, &protocol_network).await?
        } else {
            AgentPOIDispute::get_all(pool, &protocol_network).await?
        };
        Ok(disputes.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub struct POIDisputeMutation;

#[Object]
impl POIDisputeMutation {
    /// Store one or more disputes (upsert)
    async fn store_disputes(
        &self,
        ctx: &Context<'_>,
        disputes: Vec<POIDisputeInput>,
    ) -> Result<Vec<POIDispute>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let inputs: Vec<AgentPOIDisputeInput> = disputes
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        let results = AgentPOIDispute::store(pool, &inputs).await?;
        Ok(results.into_iter().map(Into::into).collect())
    }

    /// Delete disputes by allocation IDs
    async fn delete_disputes(
        &self,
        ctx: &Context<'_>,
        allocation_ids: Vec<String>,
        protocol_network: String,
    ) -> Result<i64, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let count = AgentPOIDispute::delete(pool, &allocation_ids, &protocol_network).await?;
        Ok(count as i64)
    }
}
