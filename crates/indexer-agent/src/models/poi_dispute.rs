// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

/// A POI (Proof of Indexing) dispute record
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct POIDispute {
    pub allocation_id: String,
    pub subgraph_deployment_id: String,
    pub allocation_indexer: String,
    pub allocation_amount: BigDecimal,
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
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating a POI dispute
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct POIDisputeInput {
    pub allocation_id: String,
    pub subgraph_deployment_id: String,
    pub allocation_indexer: String,
    pub allocation_amount: BigDecimal,
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

impl POIDispute {
    /// Dispute statuses
    pub const STATUS_POTENTIAL: &'static str = "potential";
    pub const STATUS_VALID: &'static str = "valid";
    pub const STATUS_INVALID: &'static str = "invalid";

    /// Get a dispute by allocation ID and protocol network
    pub async fn get(
        pool: &PgPool,
        allocation_id: &str,
        protocol_network: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "POIDisputes"
            WHERE allocation_id = $1 AND protocol_network = $2
            "#,
        )
        .bind(allocation_id)
        .bind(protocol_network)
        .fetch_optional(pool)
        .await
    }

    /// Get all disputes for a protocol network
    pub async fn get_all(pool: &PgPool, protocol_network: &str) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "POIDisputes"
            WHERE protocol_network = $1
            ORDER BY closed_epoch DESC, allocation_id
            "#,
        )
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Get disputes by status for a protocol network
    pub async fn get_by_status(
        pool: &PgPool,
        status: &str,
        protocol_network: &str,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "POIDisputes"
            WHERE status = $1 AND protocol_network = $2
            ORDER BY closed_epoch DESC, allocation_id
            "#,
        )
        .bind(status)
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Store one or more disputes (upsert)
    pub async fn store(
        pool: &PgPool,
        disputes: &[POIDisputeInput],
    ) -> Result<Vec<Self>, sqlx::Error> {
        let mut results = Vec::with_capacity(disputes.len());

        for dispute in disputes {
            let result = sqlx::query_as::<_, Self>(
                r#"
                INSERT INTO "POIDisputes" (
                    allocation_id,
                    subgraph_deployment_id,
                    allocation_indexer,
                    allocation_amount,
                    allocation_proof,
                    closed_epoch,
                    closed_epoch_reference_proof,
                    closed_epoch_start_block_hash,
                    closed_epoch_start_block_number,
                    previous_epoch_reference_proof,
                    previous_epoch_start_block_hash,
                    previous_epoch_start_block_number,
                    status,
                    protocol_network,
                    created_at,
                    updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW(), NOW()
                )
                ON CONFLICT (allocation_id, protocol_network)
                DO UPDATE SET
                    subgraph_deployment_id = $2,
                    allocation_indexer = $3,
                    allocation_amount = $4,
                    allocation_proof = $5,
                    closed_epoch = $6,
                    closed_epoch_reference_proof = $7,
                    closed_epoch_start_block_hash = $8,
                    closed_epoch_start_block_number = $9,
                    previous_epoch_reference_proof = $10,
                    previous_epoch_start_block_hash = $11,
                    previous_epoch_start_block_number = $12,
                    status = $13,
                    updated_at = NOW()
                RETURNING *
                "#,
            )
            .bind(&dispute.allocation_id)
            .bind(&dispute.subgraph_deployment_id)
            .bind(&dispute.allocation_indexer)
            .bind(&dispute.allocation_amount)
            .bind(&dispute.allocation_proof)
            .bind(dispute.closed_epoch)
            .bind(&dispute.closed_epoch_reference_proof)
            .bind(&dispute.closed_epoch_start_block_hash)
            .bind(dispute.closed_epoch_start_block_number)
            .bind(&dispute.previous_epoch_reference_proof)
            .bind(&dispute.previous_epoch_start_block_hash)
            .bind(dispute.previous_epoch_start_block_number)
            .bind(&dispute.status)
            .bind(&dispute.protocol_network)
            .fetch_one(pool)
            .await?;

            results.push(result);
        }

        Ok(results)
    }

    /// Delete disputes by allocation IDs
    pub async fn delete(
        pool: &PgPool,
        allocation_ids: &[String],
        protocol_network: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM "POIDisputes"
            WHERE allocation_id = ANY($1) AND protocol_network = $2
            "#,
        )
        .bind(allocation_ids)
        .bind(protocol_network)
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Update dispute status
    pub async fn update_status(
        pool: &PgPool,
        allocation_id: &str,
        protocol_network: &str,
        status: &str,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            UPDATE "POIDisputes"
            SET status = $3, updated_at = NOW()
            WHERE allocation_id = $1 AND protocol_network = $2
            RETURNING *
            "#,
        )
        .bind(allocation_id)
        .bind(protocol_network)
        .bind(status)
        .fetch_one(pool)
        .await
    }
}
