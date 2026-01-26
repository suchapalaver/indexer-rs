// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Type};

/// Type of identifier for indexing rules
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "identifier_type", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum IdentifierType {
    Deployment,
    Subgraph,
    #[default]
    Group,
}

/// Decision basis for indexing
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[sqlx(type_name = "indexing_decision_basis", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum IndexingDecisionBasis {
    #[default]
    Rules,
    Never,
    Always,
    Offchain,
}

/// Indexing rule for controlling allocation behavior
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct IndexingRule {
    pub id: i32,
    pub identifier: String,
    pub identifier_type: Option<IdentifierType>,
    pub allocation_amount: Option<BigDecimal>,
    pub allocation_lifetime: Option<i32>,
    pub auto_renewal: bool,
    pub parallel_allocations: Option<i32>,
    pub max_allocation_percentage: Option<f64>,
    pub min_signal: Option<BigDecimal>,
    pub max_signal: Option<BigDecimal>,
    pub min_stake: Option<BigDecimal>,
    pub min_average_query_fees: Option<BigDecimal>,
    pub custom: Option<String>,
    pub decision_basis: IndexingDecisionBasis,
    pub require_supported: bool,
    pub safety: bool,
    pub protocol_network: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Input for creating or updating an indexing rule
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexingRuleInput {
    pub identifier: String,
    pub identifier_type: Option<IdentifierType>,
    pub allocation_amount: Option<BigDecimal>,
    pub allocation_lifetime: Option<i32>,
    pub auto_renewal: Option<bool>,
    pub parallel_allocations: Option<i32>,
    pub max_allocation_percentage: Option<f64>,
    pub min_signal: Option<BigDecimal>,
    pub max_signal: Option<BigDecimal>,
    pub min_stake: Option<BigDecimal>,
    pub min_average_query_fees: Option<BigDecimal>,
    pub custom: Option<String>,
    pub decision_basis: Option<IndexingDecisionBasis>,
    pub require_supported: Option<bool>,
    pub safety: Option<bool>,
    pub protocol_network: String,
}

impl IndexingRule {
    /// The special "global" identifier used for default rules
    pub const GLOBAL_IDENTIFIER: &'static str = "global";

    /// Check if this is the global (default) rule
    pub fn is_global(&self) -> bool {
        self.identifier == Self::GLOBAL_IDENTIFIER
    }

    /// Get a single indexing rule by identifier and protocol network
    pub async fn get(
        pool: &PgPool,
        identifier: &str,
        protocol_network: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "IndexingRules"
            WHERE identifier = $1 AND protocol_network = $2
            "#,
        )
        .bind(identifier)
        .bind(protocol_network)
        .fetch_optional(pool)
        .await
    }

    /// Get all indexing rules for a protocol network
    pub async fn get_all(pool: &PgPool, protocol_network: &str) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT * FROM "IndexingRules"
            WHERE protocol_network = $1
            ORDER BY identifier
            "#,
        )
        .bind(protocol_network)
        .fetch_all(pool)
        .await
    }

    /// Get the global (default) indexing rule for a protocol network
    pub async fn get_global(
        pool: &PgPool,
        protocol_network: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        Self::get(pool, Self::GLOBAL_IDENTIFIER, protocol_network).await
    }

    /// Create or update an indexing rule (upsert)
    pub async fn set(pool: &PgPool, input: IndexingRuleInput) -> Result<Self, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            INSERT INTO "IndexingRules" (
                identifier,
                identifier_type,
                allocation_amount,
                allocation_lifetime,
                auto_renewal,
                parallel_allocations,
                max_allocation_percentage,
                min_signal,
                max_signal,
                min_stake,
                min_average_query_fees,
                custom,
                decision_basis,
                require_supported,
                safety,
                protocol_network,
                created_at,
                updated_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, NOW(), NOW()
            )
            ON CONFLICT (identifier, protocol_network)
            DO UPDATE SET
                identifier_type = COALESCE($2, "IndexingRules".identifier_type),
                allocation_amount = COALESCE($3, "IndexingRules".allocation_amount),
                allocation_lifetime = COALESCE($4, "IndexingRules".allocation_lifetime),
                auto_renewal = COALESCE($5, "IndexingRules".auto_renewal),
                parallel_allocations = COALESCE($6, "IndexingRules".parallel_allocations),
                max_allocation_percentage = COALESCE($7, "IndexingRules".max_allocation_percentage),
                min_signal = COALESCE($8, "IndexingRules".min_signal),
                max_signal = COALESCE($9, "IndexingRules".max_signal),
                min_stake = COALESCE($10, "IndexingRules".min_stake),
                min_average_query_fees = COALESCE($11, "IndexingRules".min_average_query_fees),
                custom = COALESCE($12, "IndexingRules".custom),
                decision_basis = COALESCE($13, "IndexingRules".decision_basis),
                require_supported = COALESCE($14, "IndexingRules".require_supported),
                safety = COALESCE($15, "IndexingRules".safety),
                updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(&input.identifier)
        .bind(input.identifier_type)
        .bind(&input.allocation_amount)
        .bind(input.allocation_lifetime)
        .bind(input.auto_renewal.unwrap_or(true))
        .bind(input.parallel_allocations)
        .bind(input.max_allocation_percentage)
        .bind(&input.min_signal)
        .bind(&input.max_signal)
        .bind(&input.min_stake)
        .bind(&input.min_average_query_fees)
        .bind(&input.custom)
        .bind(input.decision_basis.unwrap_or_default())
        .bind(input.require_supported.unwrap_or(true))
        .bind(input.safety.unwrap_or(true))
        .bind(&input.protocol_network)
        .fetch_one(pool)
        .await
    }

    /// Delete an indexing rule
    pub async fn delete(
        pool: &PgPool,
        identifier: &str,
        protocol_network: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM "IndexingRules"
            WHERE identifier = $1 AND protocol_network = $2
            "#,
        )
        .bind(identifier)
        .bind(protocol_network)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Delete all indexing rules for a protocol network
    pub async fn delete_all(pool: &PgPool, protocol_network: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM "IndexingRules"
            WHERE protocol_network = $1
            "#,
        )
        .bind(protocol_network)
        .execute(pool)
        .await?;

        Ok(result.rows_affected())
    }
}
