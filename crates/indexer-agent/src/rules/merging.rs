// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Rule merging logic for combining specific rules with global defaults.

use bigdecimal::BigDecimal;

use crate::models::{IdentifierType, IndexingDecisionBasis, IndexingRule};

/// An indexing rule with all optional fields resolved via global defaults.
///
/// This represents a rule after merging with the global rule, where all
/// configuration fields have definite values (either from the specific
/// rule or inherited from the global default).
#[derive(Debug, Clone)]
pub struct MergedIndexingRule {
    /// The original rule identifier (deployment hash, subgraph ID, or "global")
    pub identifier: String,
    /// Type of identifier
    pub identifier_type: IdentifierType,
    /// GRT amount to allocate
    pub allocation_amount: Option<BigDecimal>,
    /// Allocation lifetime in seconds
    pub allocation_lifetime: Option<i32>,
    /// Whether to auto-renew allocations
    pub auto_renewal: bool,
    /// Maximum parallel allocations (deprecated, max 1)
    pub parallel_allocations: Option<i32>,
    /// Maximum percentage of stake to allocate (0.0-1.0)
    pub max_allocation_percentage: Option<f64>,
    /// Minimum signal threshold
    pub min_signal: Option<BigDecimal>,
    /// Maximum signal threshold
    pub max_signal: Option<BigDecimal>,
    /// Minimum stake threshold
    pub min_stake: Option<BigDecimal>,
    /// Minimum average query fees threshold
    pub min_average_query_fees: Option<BigDecimal>,
    /// Custom JSON configuration
    pub custom: Option<String>,
    /// Decision basis for this rule
    pub decision_basis: IndexingDecisionBasis,
    /// Whether to require the deployment to be supported
    pub require_supported: bool,
    /// Safety mode
    pub safety: bool,
    /// Protocol network (CAIP-2 identifier)
    pub protocol_network: String,
}

impl MergedIndexingRule {
    /// Check if this is the global (default) rule
    pub fn is_global(&self) -> bool {
        self.identifier == IndexingRule::GLOBAL_IDENTIFIER
    }

    /// Merge a specific rule with a global rule, using global values as defaults.
    ///
    /// Fields that are `None` in the specific rule will be filled with values
    /// from the global rule. This matches the TypeScript `mergeGlobal()` behavior.
    pub fn from_rule_with_global(rule: &IndexingRule, global: Option<&IndexingRule>) -> Self {
        let global = global.filter(|g| g.identifier != rule.identifier);

        Self {
            identifier: rule.identifier.clone(),
            identifier_type: rule
                .identifier_type
                .or_else(|| global.and_then(|g| g.identifier_type))
                .unwrap_or_default(),
            allocation_amount: rule
                .allocation_amount
                .clone()
                .or_else(|| global.and_then(|g| g.allocation_amount.clone())),
            allocation_lifetime: rule
                .allocation_lifetime
                .or_else(|| global.and_then(|g| g.allocation_lifetime)),
            auto_renewal: rule.auto_renewal,
            parallel_allocations: rule
                .parallel_allocations
                .or_else(|| global.and_then(|g| g.parallel_allocations)),
            max_allocation_percentage: rule
                .max_allocation_percentage
                .or_else(|| global.and_then(|g| g.max_allocation_percentage)),
            min_signal: rule
                .min_signal
                .clone()
                .or_else(|| global.and_then(|g| g.min_signal.clone())),
            max_signal: rule
                .max_signal
                .clone()
                .or_else(|| global.and_then(|g| g.max_signal.clone())),
            min_stake: rule
                .min_stake
                .clone()
                .or_else(|| global.and_then(|g| g.min_stake.clone())),
            min_average_query_fees: rule
                .min_average_query_fees
                .clone()
                .or_else(|| global.and_then(|g| g.min_average_query_fees.clone())),
            custom: rule
                .custom
                .clone()
                .or_else(|| global.and_then(|g| g.custom.clone())),
            decision_basis: rule.decision_basis,
            require_supported: rule.require_supported,
            safety: rule.safety,
            protocol_network: rule.protocol_network.clone(),
        }
    }
}

impl From<IndexingRule> for MergedIndexingRule {
    fn from(rule: IndexingRule) -> Self {
        Self::from_rule_with_global(&rule, None)
    }
}

#[cfg(test)]
mod tests {
    use bigdecimal::FromPrimitive;

    use super::*;

    fn make_rule(identifier: &str, allocation_amount: Option<i64>) -> IndexingRule {
        IndexingRule {
            id: 1,
            identifier: identifier.to_string(),
            identifier_type: Some(IdentifierType::Deployment),
            allocation_amount: allocation_amount.map(|a| BigDecimal::from_i64(a).unwrap()),
            allocation_lifetime: None,
            auto_renewal: true,
            parallel_allocations: None,
            max_allocation_percentage: None,
            min_signal: None,
            max_signal: None,
            min_stake: None,
            min_average_query_fees: None,
            custom: None,
            decision_basis: IndexingDecisionBasis::Rules,
            require_supported: true,
            safety: true,
            protocol_network: "eip155:1".to_string(),
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn test_merge_with_global_inherits_allocation_amount() {
        let global = make_rule("global", Some(1000));
        let specific = make_rule("Qm123", None);

        let merged = MergedIndexingRule::from_rule_with_global(&specific, Some(&global));

        assert_eq!(merged.identifier, "Qm123");
        assert_eq!(
            merged.allocation_amount,
            Some(BigDecimal::from_i64(1000).unwrap())
        );
    }

    #[test]
    fn test_merge_specific_overrides_global() {
        let global = make_rule("global", Some(1000));
        let mut specific = make_rule("Qm123", Some(2000));
        specific.min_signal = Some(BigDecimal::from_i64(500).unwrap());

        let merged = MergedIndexingRule::from_rule_with_global(&specific, Some(&global));

        assert_eq!(
            merged.allocation_amount,
            Some(BigDecimal::from_i64(2000).unwrap())
        );
        assert_eq!(merged.min_signal, Some(BigDecimal::from_i64(500).unwrap()));
    }

    #[test]
    fn test_merge_without_global() {
        let specific = make_rule("Qm123", Some(1500));

        let merged = MergedIndexingRule::from_rule_with_global(&specific, None);

        assert_eq!(
            merged.allocation_amount,
            Some(BigDecimal::from_i64(1500).unwrap())
        );
    }

    #[test]
    fn test_global_rule_does_not_merge_with_itself() {
        let global = make_rule("global", Some(1000));

        let merged = MergedIndexingRule::from_rule_with_global(&global, Some(&global));

        assert!(merged.is_global());
        assert_eq!(
            merged.allocation_amount,
            Some(BigDecimal::from_i64(1000).unwrap())
        );
    }
}
