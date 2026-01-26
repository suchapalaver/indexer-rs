// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

use async_graphql::{Context, InputObject, Object, SimpleObject};
use bigdecimal::BigDecimal;
use indexer_agent::{
    IdentifierType as AgentIdentifierType, IndexingDecisionBasis as AgentDecisionBasis,
    IndexingRule as AgentIndexingRule, IndexingRuleInput as AgentIndexingRuleInput,
};
use sqlx::PgPool;

/// GraphQL representation of IndexingDecisionBasis
#[derive(Debug, Clone, Copy, async_graphql::Enum, PartialEq, Eq)]
pub enum IndexingDecisionBasis {
    Rules,
    Never,
    Always,
    Offchain,
}

impl From<AgentDecisionBasis> for IndexingDecisionBasis {
    fn from(value: AgentDecisionBasis) -> Self {
        match value {
            AgentDecisionBasis::Rules => Self::Rules,
            AgentDecisionBasis::Never => Self::Never,
            AgentDecisionBasis::Always => Self::Always,
            AgentDecisionBasis::Offchain => Self::Offchain,
        }
    }
}

impl From<IndexingDecisionBasis> for AgentDecisionBasis {
    fn from(value: IndexingDecisionBasis) -> Self {
        match value {
            IndexingDecisionBasis::Rules => Self::Rules,
            IndexingDecisionBasis::Never => Self::Never,
            IndexingDecisionBasis::Always => Self::Always,
            IndexingDecisionBasis::Offchain => Self::Offchain,
        }
    }
}

/// GraphQL representation of IdentifierType
#[derive(Debug, Clone, Copy, async_graphql::Enum, PartialEq, Eq)]
pub enum IdentifierType {
    Deployment,
    Subgraph,
    Group,
}

/// GraphQL output type for IndexingRule
#[derive(Debug, Clone, SimpleObject)]
pub struct IndexingRule {
    pub id: i32,
    pub identifier: String,
    pub identifier_type: Option<IdentifierType>,
    pub allocation_amount: Option<String>,
    pub allocation_lifetime: Option<i32>,
    pub auto_renewal: bool,
    pub parallel_allocations: Option<i32>,
    pub max_allocation_percentage: Option<f64>,
    pub min_signal: Option<String>,
    pub max_signal: Option<String>,
    pub min_stake: Option<String>,
    pub min_average_query_fees: Option<String>,
    pub custom: Option<String>,
    pub decision_basis: IndexingDecisionBasis,
    pub require_supported: bool,
    pub safety: bool,
    pub protocol_network: String,
}

impl From<AgentIndexingRule> for IndexingRule {
    fn from(rule: AgentIndexingRule) -> Self {
        Self {
            id: rule.id,
            identifier: rule.identifier,
            identifier_type: rule.identifier_type.map(|t| match t {
                AgentIdentifierType::Deployment => IdentifierType::Deployment,
                AgentIdentifierType::Subgraph => IdentifierType::Subgraph,
                AgentIdentifierType::Group => IdentifierType::Group,
            }),
            allocation_amount: rule.allocation_amount.map(|a| a.to_string()),
            allocation_lifetime: rule.allocation_lifetime,
            auto_renewal: rule.auto_renewal,
            parallel_allocations: rule.parallel_allocations,
            max_allocation_percentage: rule.max_allocation_percentage,
            min_signal: rule.min_signal.map(|s| s.to_string()),
            max_signal: rule.max_signal.map(|s| s.to_string()),
            min_stake: rule.min_stake.map(|s| s.to_string()),
            min_average_query_fees: rule.min_average_query_fees.map(|f| f.to_string()),
            custom: rule.custom,
            decision_basis: rule.decision_basis.into(),
            require_supported: rule.require_supported,
            safety: rule.safety,
            protocol_network: rule.protocol_network,
        }
    }
}

/// GraphQL input type for creating/updating IndexingRule
#[derive(Debug, Clone, InputObject)]
pub struct IndexingRuleInput {
    pub identifier: String,
    pub identifier_type: Option<IdentifierType>,
    pub allocation_amount: Option<String>,
    pub allocation_lifetime: Option<i32>,
    pub auto_renewal: Option<bool>,
    pub parallel_allocations: Option<i32>,
    pub max_allocation_percentage: Option<f64>,
    pub min_signal: Option<String>,
    pub max_signal: Option<String>,
    pub min_stake: Option<String>,
    pub min_average_query_fees: Option<String>,
    pub custom: Option<String>,
    pub decision_basis: Option<IndexingDecisionBasis>,
    pub require_supported: Option<bool>,
    pub safety: Option<bool>,
    pub protocol_network: String,
}

fn parse_decimal(s: Option<String>, field_name: &str) -> Result<Option<BigDecimal>, anyhow::Error> {
    match s {
        Some(v) => v
            .parse()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Invalid {field_name}: must be a valid decimal")),
        None => Ok(None),
    }
}

impl TryFrom<IndexingRuleInput> for AgentIndexingRuleInput {
    type Error = anyhow::Error;

    fn try_from(input: IndexingRuleInput) -> Result<Self, Self::Error> {
        Ok(Self {
            identifier: input.identifier,
            identifier_type: input.identifier_type.map(|t| match t {
                IdentifierType::Deployment => AgentIdentifierType::Deployment,
                IdentifierType::Subgraph => AgentIdentifierType::Subgraph,
                IdentifierType::Group => AgentIdentifierType::Group,
            }),
            allocation_amount: parse_decimal(input.allocation_amount, "allocation_amount")?,
            allocation_lifetime: input.allocation_lifetime,
            auto_renewal: input.auto_renewal,
            parallel_allocations: input.parallel_allocations,
            max_allocation_percentage: input.max_allocation_percentage,
            min_signal: parse_decimal(input.min_signal, "min_signal")?,
            max_signal: parse_decimal(input.max_signal, "max_signal")?,
            min_stake: parse_decimal(input.min_stake, "min_stake")?,
            min_average_query_fees: parse_decimal(
                input.min_average_query_fees,
                "min_average_query_fees",
            )?,
            custom: input.custom,
            decision_basis: input.decision_basis.map(Into::into),
            require_supported: input.require_supported,
            safety: input.safety,
            protocol_network: input.protocol_network,
        })
    }
}

#[derive(Default)]
pub struct IndexingRuleQuery;

#[Object]
impl IndexingRuleQuery {
    /// Get a single indexing rule by identifier
    async fn indexing_rule(
        &self,
        ctx: &Context<'_>,
        identifier: String,
        protocol_network: String,
    ) -> Result<Option<IndexingRule>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let rule = AgentIndexingRule::get(pool, &identifier, &protocol_network).await?;
        Ok(rule.map(Into::into))
    }

    /// Get all indexing rules for a protocol network
    async fn indexing_rules(
        &self,
        ctx: &Context<'_>,
        protocol_network: String,
    ) -> Result<Vec<IndexingRule>, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let rules = AgentIndexingRule::get_all(pool, &protocol_network).await?;
        Ok(rules.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub struct IndexingRuleMutation;

#[Object]
impl IndexingRuleMutation {
    /// Set (create or update) an indexing rule
    async fn set_indexing_rule(
        &self,
        ctx: &Context<'_>,
        rule: IndexingRuleInput,
    ) -> Result<IndexingRule, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let result = AgentIndexingRule::set(pool, rule.try_into()?).await?;
        Ok(result.into())
    }

    /// Delete an indexing rule
    async fn delete_indexing_rule(
        &self,
        ctx: &Context<'_>,
        identifier: String,
        protocol_network: String,
    ) -> Result<bool, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let deleted = AgentIndexingRule::delete(pool, &identifier, &protocol_network).await?;
        Ok(deleted)
    }

    /// Delete all indexing rules for a protocol network
    async fn delete_indexing_rules(
        &self,
        ctx: &Context<'_>,
        protocol_network: String,
    ) -> Result<i64, anyhow::Error> {
        let pool = ctx.data_unchecked::<PgPool>();
        let count = AgentIndexingRule::delete_all(pool, &protocol_network).await?;
        Ok(count as i64)
    }
}
