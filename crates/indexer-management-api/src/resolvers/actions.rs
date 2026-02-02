// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::{Context, InputObject, Object, SimpleObject};
use indexer_agent::{
    validate_action_input, Action as AgentAction, ActionFilter as AgentActionFilter,
    ActionInput as AgentActionInput, ActionStatus as AgentActionStatus,
    ActionType as AgentActionType,
};
use sqlx::PgPool;

/// GraphQL representation of ActionType
#[derive(Debug, Clone, Copy, async_graphql::Enum, PartialEq, Eq)]
pub enum ActionType {
    Allocate,
    Unallocate,
    Reallocate,
}

impl ActionType {
    /// Returns the action type as a lowercase string for validation.
    fn as_str(&self) -> &'static str {
        match self {
            Self::Allocate => "allocate",
            Self::Unallocate => "unallocate",
            Self::Reallocate => "reallocate",
        }
    }
}

impl From<AgentActionType> for ActionType {
    fn from(value: AgentActionType) -> Self {
        match value {
            AgentActionType::Allocate => Self::Allocate,
            AgentActionType::Unallocate => Self::Unallocate,
            AgentActionType::Reallocate => Self::Reallocate,
        }
    }
}

impl From<ActionType> for AgentActionType {
    fn from(value: ActionType) -> Self {
        match value {
            ActionType::Allocate => Self::Allocate,
            ActionType::Unallocate => Self::Unallocate,
            ActionType::Reallocate => Self::Reallocate,
        }
    }
}

/// GraphQL representation of ActionStatus
#[derive(Debug, Clone, Copy, async_graphql::Enum, PartialEq, Eq)]
pub enum ActionStatus {
    Queued,
    Approved,
    Pending,
    Deploying,
    Success,
    Failed,
    Canceled,
}

impl From<AgentActionStatus> for ActionStatus {
    fn from(value: AgentActionStatus) -> Self {
        match value {
            AgentActionStatus::Queued => Self::Queued,
            AgentActionStatus::Approved => Self::Approved,
            AgentActionStatus::Pending => Self::Pending,
            AgentActionStatus::Deploying => Self::Deploying,
            AgentActionStatus::Success => Self::Success,
            AgentActionStatus::Failed => Self::Failed,
            AgentActionStatus::Canceled => Self::Canceled,
        }
    }
}

impl From<ActionStatus> for AgentActionStatus {
    fn from(value: ActionStatus) -> Self {
        match value {
            ActionStatus::Queued => Self::Queued,
            ActionStatus::Approved => Self::Approved,
            ActionStatus::Pending => Self::Pending,
            ActionStatus::Deploying => Self::Deploying,
            ActionStatus::Success => Self::Success,
            ActionStatus::Failed => Self::Failed,
            ActionStatus::Canceled => Self::Canceled,
        }
    }
}

/// GraphQL output type for Action
#[derive(Debug, Clone, SimpleObject)]
pub struct Action {
    pub id: i32,
    pub action_type: ActionType,
    pub status: ActionStatus,
    pub priority: Option<i32>,
    pub deployment_id: String,
    pub allocation_id: Option<String>,
    pub amount: Option<String>,
    pub poi: Option<String>,
    pub force: Option<bool>,
    pub source: String,
    pub reason: String,
    pub transaction: Option<String>,
    pub unallocate_transaction: Option<String>,
    pub failure_reason: Option<String>,
    pub protocol_network: String,
    /// Whether this is a legacy (V1) allocation. Always false for this agent.
    pub is_legacy: bool,
    /// Public POI for unallocation (Horizon only)
    pub public_poi: Option<String>,
    /// POI block number for unallocation
    pub poi_block_number: Option<i32>,
}

impl From<AgentAction> for Action {
    fn from(action: AgentAction) -> Self {
        Self {
            id: action.id,
            action_type: action.action_type.into(),
            status: action.status.into(),
            priority: action.priority,
            deployment_id: action.deployment_id,
            allocation_id: action.allocation_id,
            amount: action.amount,
            poi: action.poi,
            force: action.force,
            source: action.source,
            reason: action.reason,
            transaction: action.transaction,
            unallocate_transaction: action.unallocate_transaction,
            failure_reason: action.failure_reason,
            protocol_network: action.protocol_network,
            is_legacy: action.is_legacy,
            public_poi: action.public_poi,
            poi_block_number: action.poi_block_number,
        }
    }
}

/// GraphQL input type for queuing actions
#[derive(Debug, Clone, InputObject)]
pub struct ActionInput {
    pub action_type: ActionType,
    pub deployment_id: String,
    pub allocation_id: Option<String>,
    pub amount: Option<String>,
    pub poi: Option<String>,
    pub force: Option<bool>,
    pub source: String,
    pub reason: String,
    pub priority: Option<i32>,
    pub protocol_network: String,
    /// Public POI for unallocation (Horizon only)
    pub public_poi: Option<String>,
    /// POI block number for unallocation
    pub poi_block_number: Option<i32>,
}

impl From<ActionInput> for AgentActionInput {
    fn from(input: ActionInput) -> Self {
        Self {
            action_type: input.action_type.into(),
            deployment_id: input.deployment_id,
            allocation_id: input.allocation_id,
            amount: input.amount,
            poi: input.poi,
            force: input.force,
            source: input.source,
            reason: input.reason,
            priority: input.priority,
            protocol_network: input.protocol_network,
            // Explicitly set to false - this agent only supports Horizon (V2) allocations.
            // Legacy (V1) actions are rejected by Action::queue().
            is_legacy: Some(false),
            public_poi: input.public_poi,
            poi_block_number: input.poi_block_number,
        }
    }
}

/// Filter for querying actions
#[derive(Debug, Clone, Default, InputObject)]
pub struct ActionFilter {
    pub id: Option<i32>,
    pub status: Option<ActionStatus>,
    pub action_type: Option<ActionType>,
    pub protocol_network: Option<String>,
}

#[derive(Default)]
pub struct ActionQuery;

#[Object]
impl ActionQuery {
    /// Get actions matching the filter
    async fn actions(
        &self,
        ctx: &Context<'_>,
        filter: Option<ActionFilter>,
        protocol_network: String,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let filter = filter.unwrap_or_default();

        let agent_filter = AgentActionFilter {
            id: filter.id,
            status: filter.status.map(Into::into),
            action_type: filter.action_type.map(Into::into),
            protocol_network: Some(protocol_network),
        };

        let actions = AgentAction::get_filtered(pool, agent_filter).await?;
        Ok(actions.into_iter().map(Into::into).collect())
    }

    /// Get queued actions for a protocol network
    async fn queued_actions(
        &self,
        ctx: &Context<'_>,
        protocol_network: String,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let actions = AgentAction::get_queued(pool, &protocol_network).await?;
        Ok(actions.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub struct ActionMutation;

#[Object]
impl ActionMutation {
    /// Queue new actions
    ///
    /// Validates all action inputs before queueing. Validation includes:
    /// - Deployment ID format (IPFS CIDv0 or bytes32 hex)
    /// - Protocol network format (CAIP-2)
    /// - Action type-specific required fields
    /// - Field format validation (allocation_id, amount)
    async fn queue_actions(
        &self,
        ctx: &Context<'_>,
        actions: Vec<ActionInput>,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
            // Validate action input at API boundary (MP-5)
            validate_action_input(
                action.action_type.as_str(),
                &action.deployment_id,
                &action.protocol_network,
                action.allocation_id.as_deref(),
                action.amount.as_deref(),
                action.poi.as_deref(),
                action.public_poi.as_deref(),
                action.poi_block_number,
                // is_legacy is always Some(false) after conversion, but we validate with None
                // since the API doesn't expose this field
                None,
            )?;

            let result = AgentAction::queue(pool, action.into()).await?;
            results.push(result.into());
        }

        Ok(results)
    }

    /// Approve queued actions
    async fn approve_actions(
        &self,
        ctx: &Context<'_>,
        ids: Vec<i32>,
        protocol_network: String,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let actions = AgentAction::approve(pool, &ids, &protocol_network).await?;
        Ok(actions.into_iter().map(Into::into).collect())
    }

    /// Cancel actions
    async fn cancel_actions(
        &self,
        ctx: &Context<'_>,
        ids: Vec<i32>,
        protocol_network: String,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let actions = AgentAction::cancel(pool, &ids, &protocol_network).await?;
        Ok(actions.into_iter().map(Into::into).collect())
    }

    /// Delete actions
    async fn delete_actions(
        &self,
        ctx: &Context<'_>,
        ids: Vec<i32>,
        protocol_network: String,
    ) -> Result<i64, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let count = AgentAction::delete(pool, &ids, &protocol_network).await?;
        Ok(count as i64)
    }
}

#[cfg(test)]
mod tests {
    use async_graphql::Request;
    use test_assets::setup_shared_test_db;
    use thegraph_core::allocation_id;

    use crate::build_schema;

    #[tokio::test]
    async fn test_queue_actions_with_explicit_poi_block_number() {
        let test_db = setup_shared_test_db().await;
        let schema = build_schema(test_db.pool).await;
        let allocation_id = allocation_id!("1234567890123456789012345678901234567890").to_string();

        let mutation = r#"
            mutation QueueActions($actions: [ActionInput!]!) {
              queueActions(actions: $actions) {
                id
                actionType
                poi
                poiBlockNumber
              }
            }
        "#;

        let variables = serde_json::json!({
            "actions": [{
                "actionType": "UNALLOCATE",
                "deploymentId": "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY",
                "allocationId": allocation_id,
                "source": "test",
                "reason": "explicit poi",
                "protocolNetwork": "eip155:1",
                "poi": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "poiBlockNumber": 10
            }]
        });

        let response = schema
            .execute(
                Request::new(mutation).variables(async_graphql::Variables::from_json(variables)),
            )
            .await;

        assert!(
            response.errors.is_empty(),
            "unexpected errors: {:?}",
            response.errors
        );

        let data = response.data.into_json().expect("response data");
        let action = &data["queueActions"][0];
        assert_eq!(action["actionType"], "UNALLOCATE");
        assert_eq!(
            action["poi"],
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        );
        assert_eq!(action["poiBlockNumber"], 10);
    }

    #[tokio::test]
    async fn test_queue_actions_persists_explicit_poi_fields() {
        let test_db = setup_shared_test_db().await;
        let pool = test_db.pool.clone();
        let schema = build_schema(pool.clone()).await;

        let mutation = r#"
            mutation QueueActions($actions: [ActionInput!]!) {
              queueActions(actions: $actions) {
                id
                actionType
                poi
                publicPoi
                poiBlockNumber
              }
            }
        "#;

        let allocation_id = allocation_id!("1234567890123456789012345678901234567890").to_string();

        let variables = serde_json::json!({
            "actions": [{
                "actionType": "UNALLOCATE",
                "deploymentId": "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY",
                "allocationId": allocation_id,
                "source": "test",
                "reason": "explicit poi",
                "protocolNetwork": "eip155:1",
                "poi": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "publicPoi": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "poiBlockNumber": 10
            }]
        });

        let response = schema
            .execute(
                Request::new(mutation).variables(async_graphql::Variables::from_json(variables)),
            )
            .await;

        assert!(
            response.errors.is_empty(),
            "unexpected errors: {:?}",
            response.errors
        );

        let data = response.data.into_json().expect("response data");
        let action = &data["queueActions"][0];
        let action_id = action["id"].as_i64().expect("action id") as i32;
        assert_eq!(action["actionType"], "UNALLOCATE");
        assert_eq!(
            action["poi"],
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        );
        assert_eq!(
            action["publicPoi"],
            "0x0000000000000000000000000000000000000000000000000000000000000002"
        );
        assert_eq!(action["poiBlockNumber"], 10);

        let row: (Option<String>, Option<String>, Option<i32>) = sqlx::query_as(
            r#"
            SELECT poi, public_poi, poi_block_number
            FROM "Actions"
            WHERE id = $1
            "#,
        )
        .bind(action_id)
        .fetch_one(&pool)
        .await
        .expect("fetch action");

        assert_eq!(
            row.0.as_deref(),
            Some("0x0000000000000000000000000000000000000000000000000000000000000001")
        );
        assert_eq!(
            row.1.as_deref(),
            Some("0x0000000000000000000000000000000000000000000000000000000000000002")
        );
        assert_eq!(row.2, Some(10));
    }
}
