// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Deployment evaluation logic for determining allocation decisions.

use std::collections::HashMap;

use bigdecimal::{BigDecimal, Zero};
use tracing::{debug, trace};

use super::merging::MergedIndexingRule;
use crate::models::{IdentifierType, IndexingDecisionBasis, IndexingRule};

/// Why a deployment was selected or rejected for allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationCriteria {
    /// No rule found for this deployment
    NotApplicable,
    /// No activation criteria were met
    None,
    /// Always allocate (decisionBasis = always)
    Always,
    /// Signal threshold was met
    SignalThreshold,
    /// Minimum stake threshold was met
    MinStake,
    /// Minimum average query fees threshold was met
    MinAverageQueryFees,
    /// Deployment is unsupported (denied) and requireSupported is true
    Unsupported,
    /// Never allocate (decisionBasis = never)
    Never,
    /// Offchain only (decisionBasis = offchain)
    Offchain,
    /// No allocation amount configured
    InvalidAllocationAmount,
}

impl std::fmt::Display for ActivationCriteria {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::NotApplicable => "not_applicable",
            Self::None => "none",
            Self::Always => "always",
            Self::SignalThreshold => "signal_threshold",
            Self::MinStake => "min_stake",
            Self::MinAverageQueryFees => "min_avg_query_fees",
            Self::Unsupported => "unsupported",
            Self::Never => "never",
            Self::Offchain => "offchain",
            Self::InvalidAllocationAmount => "invalid_allocation_amount",
        };
        write!(f, "{s}")
    }
}

/// Result of evaluating whether to allocate to a deployment.
#[derive(Debug, Clone)]
pub struct AllocationDecision {
    /// The deployment identifier (bytes32 or IPFS hash)
    pub deployment_id: String,
    /// Whether to create an allocation for this deployment
    pub to_allocate: bool,
    /// The reason for the decision
    pub criteria: ActivationCriteria,
    /// Human-readable explanation
    pub reason: String,
    /// The merged rule that was used for this decision (if any)
    pub rule: Option<MergedIndexingRule>,
}

impl AllocationDecision {
    fn new(
        deployment_id: impl Into<String>,
        to_allocate: bool,
        criteria: ActivationCriteria,
        reason: impl Into<String>,
        rule: Option<MergedIndexingRule>,
    ) -> Self {
        Self {
            deployment_id: deployment_id.into(),
            to_allocate,
            criteria,
            reason: reason.into(),
            rule,
        }
    }

    /// Create a decision indicating no applicable rule was found
    pub fn not_applicable(deployment_id: impl Into<String>) -> Self {
        Self::new(
            deployment_id,
            false,
            ActivationCriteria::NotApplicable,
            "No indexing rule found",
            None,
        )
    }

    /// Create a decision indicating invalid allocation amount
    pub fn invalid_allocation(deployment_id: impl Into<String>, rule: MergedIndexingRule) -> Self {
        Self::new(
            deployment_id,
            false,
            ActivationCriteria::InvalidAllocationAmount,
            "No allocation amount configured",
            Some(rule),
        )
    }
}

/// Deployment data from the network subgraph.
#[derive(Debug, Clone)]
pub struct NetworkDeployment {
    /// Deployment ID (bytes32 format)
    pub id: String,
    /// IPFS hash of the deployment
    pub ipfs_hash: String,
    /// Block at which the deployment was denied (0 if not denied)
    pub denied_at: i64,
    /// Total GRT staked on this deployment
    pub staked_tokens: BigDecimal,
    /// Total GRT signalled on this deployment
    pub signalled_tokens: BigDecimal,
    /// Total query fees earned by this deployment
    pub query_fees_amount: BigDecimal,
}

impl NetworkDeployment {
    /// Check if this deployment is denied/unsupported
    pub fn is_denied(&self) -> bool {
        self.denied_at > 0
    }
}

/// Pre-processed rules for efficient O(1) lookup by deployment ID.
#[derive(Debug)]
pub struct PreprocessedRules {
    /// Map from deployment bytes32 to merged rule
    deployment_rules: HashMap<String, MergedIndexingRule>,
    /// The global (default) rule, if present
    global_rule: Option<MergedIndexingRule>,
}

impl PreprocessedRules {
    /// Build a preprocessed rules lookup from a list of rules.
    ///
    /// This creates an O(1) lookup map for deployment-specific rules and
    /// merges all rules with the global default.
    pub fn from_rules(rules: &[IndexingRule]) -> Self {
        // Find the global rule first
        let global = rules
            .iter()
            .find(|r| r.identifier == IndexingRule::GLOBAL_IDENTIFIER);

        let mut deployment_rules = HashMap::new();

        for rule in rules {
            if rule.identifier == IndexingRule::GLOBAL_IDENTIFIER {
                continue;
            }

            // Only index deployment-type rules for O(1) lookup
            if rule.identifier_type == Some(IdentifierType::Deployment) {
                let merged = MergedIndexingRule::from_rule_with_global(rule, global);
                // Use the identifier directly as key (could be IPFS hash or bytes32)
                deployment_rules.insert(rule.identifier.clone(), merged);
            }
        }

        let global_rule = global.map(|g| MergedIndexingRule::from_rule_with_global(g, None));

        Self {
            deployment_rules,
            global_rule,
        }
    }

    /// Get the rule for a deployment, falling back to global if no specific rule exists.
    pub fn get_rule(&self, deployment_id: &str) -> Option<&MergedIndexingRule> {
        self.deployment_rules
            .get(deployment_id)
            .or(self.global_rule.as_ref())
    }

    /// Get the global rule
    pub fn global_rule(&self) -> Option<&MergedIndexingRule> {
        self.global_rule.as_ref()
    }
}

/// Evaluate whether a single deployment is worth allocating towards.
///
/// This implements the core decision logic from the TypeScript
/// `isDeploymentWorthAllocatingTowards` function.
pub fn is_deployment_worth_allocating(
    deployment: &NetworkDeployment,
    rules: &PreprocessedRules,
) -> AllocationDecision {
    let deployment_id = &deployment.id;

    // Try to find a matching rule (deployment-specific or global)
    let Some(rule) = rules.get_rule(deployment_id) else {
        trace!(
            deployment = %deployment_id,
            "No indexing rule found for deployment"
        );
        return AllocationDecision::not_applicable(deployment_id);
    };

    // Check if allocation amount is configured
    if rule.allocation_amount.is_none()
        || rule
            .allocation_amount
            .as_ref()
            .map(|a| a.is_zero())
            .unwrap_or(true)
    {
        trace!(
            deployment = %deployment_id,
            rule = %rule.identifier,
            "No allocation amount configured"
        );
        return AllocationDecision::invalid_allocation(deployment_id, rule.clone());
    }

    // Check if deployment is unsupported and rule requires support
    if deployment.is_denied() && rule.require_supported {
        debug!(
            deployment = %deployment_id,
            denied_at = deployment.denied_at,
            "Deployment is unsupported"
        );
        return AllocationDecision::new(
            deployment_id,
            false,
            ActivationCriteria::Unsupported,
            format!(
                "Deployment denied at block {} and requireSupported is true",
                deployment.denied_at
            ),
            Some(rule.clone()),
        );
    }

    // Evaluate based on decision basis
    match rule.decision_basis {
        IndexingDecisionBasis::Always => {
            debug!(
                deployment = %deployment_id,
                "Allocating: decisionBasis is 'always'"
            );
            AllocationDecision::new(
                deployment_id,
                true,
                ActivationCriteria::Always,
                "decisionBasis is 'always'",
                Some(rule.clone()),
            )
        }

        IndexingDecisionBasis::Never => {
            debug!(
                deployment = %deployment_id,
                "Not allocating: decisionBasis is 'never'"
            );
            AllocationDecision::new(
                deployment_id,
                false,
                ActivationCriteria::Never,
                "decisionBasis is 'never'",
                Some(rule.clone()),
            )
        }

        IndexingDecisionBasis::Offchain => {
            debug!(
                deployment = %deployment_id,
                "Not allocating: decisionBasis is 'offchain'"
            );
            AllocationDecision::new(
                deployment_id,
                false,
                ActivationCriteria::Offchain,
                "decisionBasis is 'offchain' (sync but don't allocate)",
                Some(rule.clone()),
            )
        }

        IndexingDecisionBasis::Rules => {
            // Evaluate threshold-based criteria in priority order
            evaluate_thresholds(deployment, rule)
        }
    }
}

/// Evaluate threshold-based activation criteria.
fn evaluate_thresholds(
    deployment: &NetworkDeployment,
    rule: &MergedIndexingRule,
) -> AllocationDecision {
    let deployment_id = &deployment.id;

    // Check minStake threshold
    if let Some(ref min_stake) = rule.min_stake {
        if &deployment.staked_tokens >= min_stake {
            debug!(
                deployment = %deployment_id,
                staked = %deployment.staked_tokens,
                threshold = %min_stake,
                "Allocating: minStake threshold met"
            );
            return AllocationDecision::new(
                deployment_id,
                true,
                ActivationCriteria::MinStake,
                format!(
                    "Staked tokens ({}) >= minStake threshold ({})",
                    deployment.staked_tokens, min_stake
                ),
                Some(rule.clone()),
            );
        }
    }

    // Check minSignal threshold
    if let Some(ref min_signal) = rule.min_signal {
        if &deployment.signalled_tokens >= min_signal {
            debug!(
                deployment = %deployment_id,
                signalled = %deployment.signalled_tokens,
                threshold = %min_signal,
                "Allocating: signal threshold met"
            );
            return AllocationDecision::new(
                deployment_id,
                true,
                ActivationCriteria::SignalThreshold,
                format!(
                    "Signalled tokens ({}) >= minSignal threshold ({})",
                    deployment.signalled_tokens, min_signal
                ),
                Some(rule.clone()),
            );
        }
    }

    // Check minAverageQueryFees threshold
    if let Some(ref min_avg_fees) = rule.min_average_query_fees {
        if &deployment.query_fees_amount >= min_avg_fees {
            debug!(
                deployment = %deployment_id,
                query_fees = %deployment.query_fees_amount,
                threshold = %min_avg_fees,
                "Allocating: minAverageQueryFees threshold met"
            );
            return AllocationDecision::new(
                deployment_id,
                true,
                ActivationCriteria::MinAverageQueryFees,
                format!(
                    "Query fees ({}) >= minAverageQueryFees threshold ({})",
                    deployment.query_fees_amount, min_avg_fees
                ),
                Some(rule.clone()),
            );
        }
    }

    // No criteria met
    debug!(
        deployment = %deployment_id,
        staked = %deployment.staked_tokens,
        signalled = %deployment.signalled_tokens,
        query_fees = %deployment.query_fees_amount,
        "Not allocating: no threshold criteria met"
    );
    AllocationDecision::new(
        deployment_id,
        false,
        ActivationCriteria::None,
        "No activation criteria met",
        Some(rule.clone()),
    )
}

/// Evaluate multiple deployments against the indexing rules.
///
/// Returns allocation decisions for all deployments.
pub fn evaluate_deployments(
    deployments: &[NetworkDeployment],
    rules: &PreprocessedRules,
) -> Vec<AllocationDecision> {
    deployments
        .iter()
        .map(|d| is_deployment_worth_allocating(d, rules))
        .collect()
}

#[cfg(test)]
mod tests {
    use bigdecimal::FromPrimitive;

    use super::*;

    fn make_rule(
        identifier: &str,
        allocation_amount: Option<i64>,
        decision_basis: IndexingDecisionBasis,
    ) -> IndexingRule {
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
            decision_basis,
            require_supported: true,
            safety: true,
            protocol_network: "eip155:1".to_string(),
            created_at: None,
            updated_at: None,
        }
    }

    fn make_deployment(id: &str, denied: bool) -> NetworkDeployment {
        NetworkDeployment {
            id: id.to_string(),
            ipfs_hash: format!("Qm{id}"),
            denied_at: if denied { 12345 } else { 0 },
            staked_tokens: BigDecimal::from_i64(1000).unwrap(),
            signalled_tokens: BigDecimal::from_i64(500).unwrap(),
            query_fees_amount: BigDecimal::from_i64(100).unwrap(),
        }
    }

    #[test]
    fn test_no_rule_returns_not_applicable() {
        let rules = PreprocessedRules::from_rules(&[]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::NotApplicable);
    }

    #[test]
    fn test_always_allocates() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            Some(1000),
            IndexingDecisionBasis::Always,
        )]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::Always);
    }

    #[test]
    fn test_never_does_not_allocate() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            Some(1000),
            IndexingDecisionBasis::Never,
        )]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::Never);
    }

    #[test]
    fn test_offchain_does_not_allocate() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            Some(1000),
            IndexingDecisionBasis::Offchain,
        )]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::Offchain);
    }

    #[test]
    fn test_denied_deployment_rejected_when_require_supported() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            Some(1000),
            IndexingDecisionBasis::Always,
        )]);
        let deployment = make_deployment("dep1", true);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::Unsupported);
    }

    #[test]
    fn test_no_allocation_amount_returns_invalid() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            None,
            IndexingDecisionBasis::Always,
        )]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(
            decision.criteria,
            ActivationCriteria::InvalidAllocationAmount
        );
    }

    #[test]
    fn test_min_stake_threshold() {
        let mut rule = make_rule("global", Some(1000), IndexingDecisionBasis::Rules);
        rule.min_stake = Some(BigDecimal::from_i64(500).unwrap()); // deployment has 1000

        let rules = PreprocessedRules::from_rules(&[rule]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::MinStake);
    }

    #[test]
    fn test_min_signal_threshold() {
        let mut rule = make_rule("global", Some(1000), IndexingDecisionBasis::Rules);
        rule.min_signal = Some(BigDecimal::from_i64(200).unwrap()); // deployment has 500

        let rules = PreprocessedRules::from_rules(&[rule]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::SignalThreshold);
    }

    #[test]
    fn test_min_average_query_fees_threshold() {
        let mut rule = make_rule("global", Some(1000), IndexingDecisionBasis::Rules);
        rule.min_average_query_fees = Some(BigDecimal::from_i64(50).unwrap()); // deployment has 100

        let rules = PreprocessedRules::from_rules(&[rule]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::MinAverageQueryFees);
    }

    #[test]
    fn test_no_threshold_met() {
        let mut rule = make_rule("global", Some(1000), IndexingDecisionBasis::Rules);
        rule.min_stake = Some(BigDecimal::from_i64(10000).unwrap()); // deployment only has 1000

        let rules = PreprocessedRules::from_rules(&[rule]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(!decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::None);
    }

    #[test]
    fn test_deployment_specific_rule_overrides_global() {
        let global = make_rule("global", Some(1000), IndexingDecisionBasis::Never);
        let mut specific = make_rule("dep1", Some(2000), IndexingDecisionBasis::Always);
        specific.identifier_type = Some(IdentifierType::Deployment);

        let rules = PreprocessedRules::from_rules(&[global, specific]);
        let deployment = make_deployment("dep1", false);

        let decision = is_deployment_worth_allocating(&deployment, &rules);

        assert!(decision.to_allocate);
        assert_eq!(decision.criteria, ActivationCriteria::Always);
    }

    #[test]
    fn test_evaluate_deployments() {
        let rules = PreprocessedRules::from_rules(&[make_rule(
            "global",
            Some(1000),
            IndexingDecisionBasis::Always,
        )]);

        let deployments = vec![
            make_deployment("dep1", false),
            make_deployment("dep2", false),
            make_deployment("dep3", true), // denied
        ];

        let decisions = evaluate_deployments(&deployments, &rules);

        assert_eq!(decisions.len(), 3);
        assert!(decisions[0].to_allocate);
        assert!(decisions[1].to_allocate);
        assert!(!decisions[2].to_allocate); // rejected due to denied
    }
}
