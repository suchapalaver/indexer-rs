// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::{Context, InputObject, Object, SimpleObject};
use indexer_agent::{
    Action as AgentAction, ActionFilter as AgentActionFilter, ActionInput as AgentActionInput,
    ActionStatus as AgentActionStatus, ActionType as AgentActionType,
};
use sqlx::PgPool;

/// GraphQL representation of ActionType
#[derive(Debug, Clone, Copy, async_graphql::Enum, PartialEq, Eq)]
pub enum ActionType {
    Allocate,
    Unallocate,
    Reallocate,
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
    pub failure_reason: Option<String>,
    pub protocol_network: String,
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
            failure_reason: action.failure_reason,
            protocol_network: action.protocol_network,
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
            is_legacy: None,
            public_poi: None,
            poi_block_number: None,
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
    async fn queue_actions(
        &self,
        ctx: &Context<'_>,
        actions: Vec<ActionInput>,
    ) -> Result<Vec<Action>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let mut results = Vec::with_capacity(actions.len());

        for action in actions {
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
