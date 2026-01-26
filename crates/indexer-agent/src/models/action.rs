// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Type};

/// Type of allocation action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "action_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    Allocate,
    Unallocate,
    Reallocate,
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

impl Action {
    /// Queue a new action
    pub async fn queue(pool: &PgPool, input: ActionInput) -> Result<Self, sqlx::Error> {
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
        .bind(input.is_legacy.unwrap_or(true))
        .bind(&input.public_poi)
        .bind(input.poi_block_number)
        .fetch_one(pool)
        .await
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
        // Build dynamic query based on filter
        let mut query = String::from(r#"SELECT * FROM "Actions" WHERE 1=1"#);

        if filter.id.is_some() {
            query.push_str(" AND id = $1");
        }
        if filter.status.is_some() {
            query.push_str(" AND status = $2");
        }
        if filter.action_type.is_some() {
            query.push_str(" AND type = $3");
        }
        if filter.protocol_network.is_some() {
            query.push_str(" AND protocol_network = $4");
        }

        query.push_str(" ORDER BY priority DESC, created_at ASC");

        sqlx::query_as::<_, Self>(&query)
            .bind(filter.id)
            .bind(filter.status)
            .bind(filter.action_type)
            .bind(&filter.protocol_network)
            .fetch_all(pool)
            .await
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

    /// Update action status
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
            SET status = $3, transaction = COALESCE($4, transaction),
                failure_reason = COALESCE($5, failure_reason), updated_at = NOW()
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
}
