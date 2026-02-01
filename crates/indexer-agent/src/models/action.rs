// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Type};
use thiserror::Error;

use crate::{
    error::{ErrorClass, ErrorClassification},
    validation::{validate_action_input, ValidationError},
};

/// Errors that can occur when working with actions.
#[derive(Debug, Error)]
pub enum ActionError {
    /// A non-terminal action already exists for this deployment.
    ///
    /// This occurs when attempting to queue an action for a deployment that
    /// already has a pending action (queued, approved, pending, or deploying).
    #[error("action already pending for deployment {deployment_id}")]
    DuplicatePendingAction { deployment_id: String },

    /// Legacy actions are not supported.
    ///
    /// This agent only supports Horizon (V2) allocations. Legacy actions
    /// from the V1 Staking contract cannot be executed.
    #[error("legacy actions are not supported; this agent only supports Horizon (V2) allocations")]
    LegacyActionNotSupported,
    /// Input validation failed.
    #[error("invalid action input: {0}")]
    InvalidInput(#[from] ValidationError),

    /// Database error.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl ErrorClassification for ActionError {
    fn class(&self) -> ErrorClass {
        match self {
            ActionError::DuplicatePendingAction { .. } => ErrorClass::Invariant,
            ActionError::LegacyActionNotSupported => ErrorClass::Input,
            ActionError::InvalidInput(_) => ErrorClass::Input,
            ActionError::Database(_) => ErrorClass::External,
        }
    }
}

/// Type of allocation action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "action_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Allocate,
    Unallocate,
    Reallocate,
}

impl ActionType {
    /// Returns the action type as a lowercase string slice.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ActionType::Allocate => "allocate",
            ActionType::Unallocate => "unallocate",
            ActionType::Reallocate => "reallocate",
        }
    }
}

/// Status of an action in the queue
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "action_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    #[default]
    Queued,
    Approved,
    Pending,
    Deploying,
    Success,
    Failed,
    Canceled,
}

/// An action in the allocation queue
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Action {
    pub id: i32,
    #[sqlx(rename = "type")]
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
    pub is_legacy: bool,
    pub public_poi: Option<String>,
    pub poi_block_number: Option<i32>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for queuing a new action
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub is_legacy: Option<bool>,
    pub public_poi: Option<String>,
    pub poi_block_number: Option<i32>,
}

/// Filter for querying actions
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionFilter {
    pub id: Option<i32>,
    pub status: Option<ActionStatus>,
    pub action_type: Option<ActionType>,
    pub protocol_network: Option<String>,
}

/// Name of the partial unique index for duplicate action prevention.
const DUPLICATE_ACTION_INDEX: &str = "idx_one_pending_action_per_deployment";

impl Action {
    /// Check if an allocation ID is already present in the action queue for a protocol network.
    pub async fn allocation_id_exists(
        pool: &PgPool,
        protocol_network: &str,
        allocation_id: &str,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT 1 FROM "Actions"
            WHERE allocation_id = $1 AND protocol_network = $2
            LIMIT 1
            "#,
        )
        .bind(allocation_id)
        .bind(protocol_network)
        .fetch_optional(pool)
        .await?;

        Ok(exists.is_some())
    }
    /// Queue a new action.
    ///
    /// Returns `ActionError::DuplicatePendingAction` if a non-terminal action
    /// already exists for this deployment (enforced by database constraint).
    ///
    /// Returns `ActionError::LegacyActionNotSupported` if the action is marked
    /// as legacy (is_legacy = true). This agent only supports Horizon (V2)
    /// allocations.
    pub async fn queue(pool: &PgPool, input: ActionInput) -> Result<Self, ActionError> {
        // Validate input and reject legacy actions (Horizon-only).
        validate_action_input(
            input.action_type.as_str(),
            &input.deployment_id,
            &input.protocol_network,
            input.allocation_id.as_deref(),
            input.amount.as_deref(),
            input.poi.as_deref(),
            input.public_poi.as_deref(),
            input.poi_block_number,
            input.is_legacy,
        )?;

        let deployment_id = input.deployment_id.clone();

        sqlx::query_as::<_, Self>(
            r#"
            INSERT INTO "Actions" (
                type,
                status,
                priority,
                deployment_id,
                allocation_id,
                amount,
                poi,
                force,
                source,
                reason,
                protocol_network,
                is_legacy,
                public_poi,
                poi_block_number,
                created_at,
                updated_at
            ) VALUES (
                $1, 'queued', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NOW(), NOW()
            )
            RETURNING *
            "#,
        )
        .bind(input.action_type)
        .bind(input.priority.unwrap_or(0))
        .bind(&input.deployment_id)
        .bind(&input.allocation_id)
        .bind(&input.amount)
        .bind(&input.poi)
        .bind(input.force)
        .bind(&input.source)
        .bind(&input.reason)
        .bind(&input.protocol_network)
        .bind(input.is_legacy.unwrap_or(false))
        .bind(&input.public_poi)
        .bind(input.poi_block_number)
        .fetch_one(pool)
        .await
        .map_err(|e| {
            // Check if this is a unique constraint violation from our index
            if e.to_string().contains(DUPLICATE_ACTION_INDEX) {
                ActionError::DuplicatePendingAction { deployment_id }
            } else {
                ActionError::Database(e)
            }
        })
    }

    /// Get an action by ID and protocol network
    pub async fn get(
        pool: &PgPool,
        id: i32,
        protocol_network: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "Actions"
            WHERE id = $1 AND protocol_network = $2
            "#,
        )
        .bind(id)
        .bind(protocol_network)
        .fetch_optional(pool)
        .await
    }

    /// Get actions matching the filter
    pub async fn get_filtered(
        pool: &PgPool,
        filter: ActionFilter,
    ) -> Result<Vec<Self>, sqlx::Error> {
        // Build dynamic query based on filter with correct parameter indices
        let mut query = String::from(r#"SELECT * FROM "Actions" WHERE 1=1"#);
        let mut param_idx = 1u32;

        if filter.id.is_some() {
            query.push_str(&format!(" AND id = ${param_idx}"));
            param_idx += 1;
        }
        if filter.status.is_some() {
            query.push_str(&format!(" AND status = ${param_idx}"));
            param_idx += 1;
        }
        if filter.action_type.is_some() {
            query.push_str(&format!(" AND type = ${param_idx}"));
            param_idx += 1;
        }
        if filter.protocol_network.is_some() {
            query.push_str(&format!(" AND protocol_network = ${param_idx}"));
            // param_idx += 1; // Not needed for last parameter
        }

        query.push_str(" ORDER BY priority DESC, created_at ASC");

        // Build query with only the parameters that are present
        let mut query_builder = sqlx::query_as::<_, Self>(&query);

        if let Some(id) = filter.id {
            query_builder = query_builder.bind(id);
        }
        if let Some(status) = filter.status {
            query_builder = query_builder.bind(status);
        }
        if let Some(action_type) = filter.action_type {
            query_builder = query_builder.bind(action_type);
        }
        if let Some(ref protocol_network) = filter.protocol_network {
            query_builder = query_builder.bind(protocol_network);
        }

        query_builder.fetch_all(pool).await
    }

    /// Get all queued actions for a protocol network
    pub async fn get_queued(
        pool: &PgPool,
        protocol_network: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "Actions"
            WHERE status = 'queued' AND protocol_network = $1
            ORDER BY priority DESC, created_at ASC
            "#,
        )
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Get all approved actions for a protocol network (ready for execution)
    pub async fn get_approved(
        pool: &PgPool,
        protocol_network: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "Actions"
            WHERE status = 'approved' AND protocol_network = $1
            ORDER BY priority DESC, created_at ASC
            "#,
        )
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Approve actions (move from queued to approved)
    pub async fn approve(
        pool: &PgPool,
        ids: &[i32],
        protocol_network: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            UPDATE "Actions"
            SET status = 'approved', updated_at = NOW()
            WHERE id = ANY($1) AND protocol_network = $2 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(ids)
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Cancel actions
    pub async fn cancel(
        pool: &PgPool,
        ids: &[i32],
        protocol_network: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            UPDATE "Actions"
            SET status = 'canceled', updated_at = NOW()
            WHERE id = ANY($1) AND protocol_network = $2 AND status IN ('queued', 'approved')
            RETURNING *
            "#,
        )
        .bind(ids)
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Update action status.
    ///
    /// # Transaction Hash Immutability
    ///
    /// Once a transaction hash is set, it cannot be overwritten. This preserves
    /// the audit trail and ensures clarity about which transaction was executed.
    /// If a transaction hash is provided but one already exists, the existing
    /// hash is preserved and the new value is ignored.
    ///
    /// # Failure Reason
    ///
    /// Failure reasons can be updated on retry, so new values overwrite existing
    /// ones (unlike transaction hashes).
    pub async fn update_status(
        pool: &PgPool,
        id: i32,
        protocol_network: &str,
        status: ActionStatus,
        transaction: Option<&str>,
        failure_reason: Option<&str>,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            UPDATE "Actions"
            SET status = $3,
                -- Transaction hash is immutable: preserve existing value if set
                transaction = COALESCE(transaction, $4),
                failure_reason = COALESCE($5, failure_reason),
                updated_at = NOW()
            WHERE id = $1 AND protocol_network = $2
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(protocol_network)
        .bind(status)
        .bind(transaction)
        .bind(failure_reason)
        .fetch_one(pool)
        .await
    }

    /// Delete actions
    pub async fn delete(
        pool: &PgPool,
        ids: &[i32],
        protocol_network: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM "Actions"
            WHERE id = ANY($1) AND protocol_network = $2
            "#,
        )
        .bind(ids)
        .bind(protocol_network)
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Check if an action was recently executed (succeeded or failed) for this deployment.
    ///
    /// Returns `true` if an action for the given deployment completed within the cooldown
    /// period, preventing rapid re-queueing of actions for the same deployment.
    ///
    /// This implements Invariant 8.2/22.2: Recently Executed Check from the TypeScript
    /// agent, which uses a 15-minute default cooldown window.
    ///
    /// # Arguments
    /// * `pool` - Database connection pool
    /// * `deployment_id` - The deployment to check
    /// * `protocol_network` - The protocol network identifier
    /// * `cooldown_seconds` - The cooldown period in seconds
    ///
    /// # Returns
    /// `true` if a recent action exists (cooldown not expired), `false` otherwise
    pub async fn was_recently_executed(
        pool: &PgPool,
        deployment_id: &str,
        protocol_network: &str,
        cooldown_seconds: u64,
    ) -> Result<bool, sqlx::Error> {
        // Calculate the cutoff timestamp
        let cutoff_interval = format!("{cooldown_seconds} seconds");

        let result: (bool,) = sqlx::query_as(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM "Actions"
                WHERE deployment_id = $1
                  AND protocol_network = $2
                  AND status IN ('success', 'failed')
                  AND updated_at > NOW() - $3::interval
            )
            "#,
        )
        .bind(deployment_id)
        .bind(protocol_network)
        .bind(&cutoff_interval)
        .fetch_one(pool)
        .await?;

        Ok(result.0)
    }
}

#[cfg(test)]
mod tests {
    use test_assets::setup_shared_test_db;

    use super::*;

    #[test]
    fn test_action_error_duplicate_pending() {
        let error = ActionError::DuplicatePendingAction {
            deployment_id: "Qm123abc".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "action already pending for deployment Qm123abc"
        );
    }

    #[test]
    fn test_action_error_database() {
        // ActionError::Database wraps sqlx::Error, which we can't easily construct
        // in a unit test, but we verify the From impl exists by checking the type
        let _: fn(sqlx::Error) -> ActionError = ActionError::from;
    }

    #[tokio::test]
    async fn test_action_transaction_immutable() {
        let test_db = setup_shared_test_db().await;
        let pool = test_db.pool;

        let input = ActionInput {
            action_type: ActionType::Allocate,
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: None,
            amount: Some("1".to_string()),
            poi: None,
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            priority: Some(0),
            protocol_network: "eip155:1".to_string(),
            is_legacy: Some(false),
            public_poi: None,
            poi_block_number: None,
        };

        let action = Action::queue(&pool, input).await.unwrap();

        let action = Action::update_status(
            &pool,
            action.id,
            &action.protocol_network,
            ActionStatus::Success,
            Some("0xabc"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(action.transaction.as_deref(), Some("0xabc"));

        let action = Action::update_status(
            &pool,
            action.id,
            &action.protocol_network,
            ActionStatus::Success,
            Some("0xdef"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(action.transaction.as_deref(), Some("0xabc"));
    }

    #[tokio::test]
    async fn test_action_queue_rejects_missing_amount() {
        let test_db = setup_shared_test_db().await;
        let pool = test_db.pool;

        let input = ActionInput {
            action_type: ActionType::Allocate,
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: None,
            amount: None,
            poi: None,
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            priority: Some(0),
            protocol_network: "eip155:1".to_string(),
            is_legacy: Some(false),
            public_poi: None,
            poi_block_number: None,
        };

        let err = Action::queue(&pool, input).await.unwrap_err();
        assert!(matches!(
            err,
            ActionError::InvalidInput(ValidationError::MissingRequiredField { .. })
        ));
    }

    #[tokio::test]
    async fn test_action_queue_rejects_missing_allocation_id() {
        let test_db = setup_shared_test_db().await;
        let pool = test_db.pool;

        let input = ActionInput {
            action_type: ActionType::Unallocate,
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: None,
            amount: None,
            poi: None,
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            priority: Some(0),
            protocol_network: "eip155:1".to_string(),
            is_legacy: Some(false),
            public_poi: None,
            poi_block_number: None,
        };

        let err = Action::queue(&pool, input).await.unwrap_err();
        assert!(matches!(
            err,
            ActionError::InvalidInput(ValidationError::MissingRequiredField { .. })
        ));
    }

    #[tokio::test]
    async fn test_action_queue_rejects_legacy() {
        let test_db = setup_shared_test_db().await;
        let pool = test_db.pool;

        let input = ActionInput {
            action_type: ActionType::Allocate,
            deployment_id: "0x0000000000000000000000000000000000000000000000000000000000000001"
                .to_string(),
            allocation_id: None,
            amount: Some("1".to_string()),
            poi: None,
            force: None,
            source: "test".to_string(),
            reason: "test".to_string(),
            priority: Some(0),
            protocol_network: "eip155:1".to_string(),
            is_legacy: Some(true),
            public_poi: None,
            poi_block_number: None,
        };

        let err = Action::queue(&pool, input).await.unwrap_err();
        assert!(matches!(
            err,
            ActionError::InvalidInput(ValidationError::LegacyActionNotSupported)
        ));
    }
}
