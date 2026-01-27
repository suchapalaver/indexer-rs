// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Deployment-level reconciliation logic.
//!
//! Compares the desired allocation state (from rules evaluation) against
//! the current allocation state (from the network) and queues appropriate actions.

use std::collections::HashMap;

use tracing::{debug, info, trace};

use super::{
    actions::{queue_allocation_action, queue_reallocation_action, queue_unallocation_action},
    context::{ActiveAllocation, AllocationsByDeployment, ReconciliationContext},
};
use crate::{
    models::Action,
    rules::{AllocationDecision, PreprocessedRules},
};

/// Result of reconciling a single deployment.
#[derive(Debug)]
pub struct DeploymentReconciliation {
    /// Deployment ID
    pub deployment_id: String,
    /// Whether an allocation should exist
    pub should_allocate: bool,
    /// Current allocations for this deployment
    pub current_allocations: Vec<ActiveAllocation>,
    /// Actions that were queued
    pub actions_queued: Vec<Action>,
    /// Reason for the reconciliation outcome
    pub reason: String,
}

/// Reconcile allocation state for all deployments.
///
/// This is the main entry point for deployment reconciliation. It:
/// 1. Groups active allocations by deployment
/// 2. For each deployment decision, compares desired vs current state
/// 3. Queues appropriate actions (allocate/unallocate/reallocate)
pub async fn reconcile_deployment_allocations(
    ctx: &ReconciliationContext,
    decisions: &[AllocationDecision],
    active_allocations: &[ActiveAllocation],
    rules: &PreprocessedRules,
) -> Result<Vec<DeploymentReconciliation>, sqlx::Error> {
    // Group allocations by deployment
    let allocations_by_deployment = group_allocations_by_deployment(active_allocations);

    info!(
        decisions = decisions.len(),
        active_allocations = active_allocations.len(),
        unique_deployments = allocations_by_deployment.len(),
        "Starting deployment reconciliation"
    );

    let mut results = Vec::new();

    for decision in decisions {
        let current_allocations = allocations_by_deployment
            .get(&decision.deployment_id)
            .cloned()
            .unwrap_or_default();

        let result =
            reconcile_single_deployment(ctx, decision, &current_allocations, rules).await?;

        results.push(result);
    }

    // Also check for allocations on deployments that have no rule (should be closed)
    for (deployment_id, allocations) in &allocations_by_deployment {
        let has_decision = decisions.iter().any(|d| &d.deployment_id == deployment_id);
        if !has_decision && !allocations.is_empty() {
            debug!(
                deployment = %deployment_id,
                allocations = allocations.len(),
                "Found allocations for deployment with no rule, closing"
            );

            let mut actions_queued = Vec::new();
            for alloc in allocations {
                if let Some(action) = queue_unallocation_action(
                    &ctx.pool,
                    &ctx.protocol_network,
                    deployment_id,
                    &alloc.id.to_string(),
                    "No indexing rule found for deployment",
                    alloc.is_legacy,
                    ctx.auto_approve,
                )
                .await?
                {
                    actions_queued.push(action);
                }
            }

            results.push(DeploymentReconciliation {
                deployment_id: deployment_id.clone(),
                should_allocate: false,
                current_allocations: allocations.clone(),
                actions_queued,
                reason: "No indexing rule found".to_string(),
            });
        }
    }

    let actions_count: usize = results.iter().map(|r| r.actions_queued.len()).sum();
    info!(
        deployments_processed = results.len(),
        actions_queued = actions_count,
        "Deployment reconciliation complete"
    );

    Ok(results)
}

/// Reconcile a single deployment's allocation state.
async fn reconcile_single_deployment(
    ctx: &ReconciliationContext,
    decision: &AllocationDecision,
    current_allocations: &[ActiveAllocation],
    _rules: &PreprocessedRules,
) -> Result<DeploymentReconciliation, sqlx::Error> {
    let deployment_id = &decision.deployment_id;
    let mut actions_queued = Vec::new();

    trace!(
        deployment = %deployment_id,
        should_allocate = decision.to_allocate,
        current_allocations = current_allocations.len(),
        criteria = %decision.criteria,
        "Reconciling deployment"
    );

    if decision.to_allocate {
        // Should have an allocation
        if current_allocations.is_empty() {
            // No allocation exists, create one
            if let Some(action) = queue_allocation_action(
                &ctx.pool,
                &ctx.protocol_network,
                decision,
                ctx.auto_approve,
            )
            .await?
            {
                actions_queued.push(action);
            }
        } else {
            // Allocation exists, check if it needs renewal
            for alloc in current_allocations {
                if should_reallocate(ctx, alloc, decision) {
                    if let Some(ref rule) = decision.rule {
                        if rule.auto_renewal {
                            let amount = rule
                                .allocation_amount
                                .as_ref()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| alloc.allocated_tokens.clone());

                            if let Some(action) = queue_reallocation_action(
                                &ctx.pool,
                                &ctx.protocol_network,
                                deployment_id,
                                &alloc.id.to_string(),
                                &amount,
                                alloc.is_legacy,
                                ctx.auto_approve,
                            )
                            .await?
                            {
                                actions_queued.push(action);
                            }
                        } else {
                            debug!(
                                deployment = %deployment_id,
                                allocation = %alloc.id,
                                "Allocation expiring but auto_renewal is disabled"
                            );
                        }
                    }
                }
            }
        }
    } else {
        // Should NOT have an allocation - close any existing ones
        for alloc in current_allocations {
            if let Some(action) = queue_unallocation_action(
                &ctx.pool,
                &ctx.protocol_network,
                deployment_id,
                &alloc.id.to_string(),
                &decision.reason,
                alloc.is_legacy,
                ctx.auto_approve,
            )
            .await?
            {
                actions_queued.push(action);
            }
        }
    }

    Ok(DeploymentReconciliation {
        deployment_id: deployment_id.clone(),
        should_allocate: decision.to_allocate,
        current_allocations: current_allocations.to_vec(),
        actions_queued,
        reason: decision.reason.clone(),
    })
}

/// Check if an allocation should be reallocated (approaching max lifetime).
fn should_reallocate(
    ctx: &ReconciliationContext,
    allocation: &ActiveAllocation,
    decision: &AllocationDecision,
) -> bool {
    should_reallocate_with_params(
        ctx.current_epoch,
        ctx.max_allocation_epochs,
        allocation,
        decision,
    )
}

/// Check if an allocation should be reallocated, with explicit parameters.
///
/// This is the core logic, separated for easier testing.
fn should_reallocate_with_params(
    current_epoch: u64,
    max_allocation_epochs: u64,
    allocation: &ActiveAllocation,
    decision: &AllocationDecision,
) -> bool {
    let Some(ref rule) = decision.rule else {
        return false;
    };

    // Determine the desired allocation lifetime
    let desired_lifetime = rule
        .allocation_lifetime
        .map(|l| l as u64)
        .unwrap_or(max_allocation_epochs);

    // Calculate allocation age in epochs
    let allocation_age = current_epoch.saturating_sub(allocation.created_at_epoch);

    // Reallocate if allocation has reached its desired lifetime
    let should_reallocate = allocation_age >= desired_lifetime;

    if should_reallocate {
        debug!(
            allocation = %allocation.id,
            deployment = %decision.deployment_id,
            age_epochs = allocation_age,
            max_epochs = desired_lifetime,
            "Allocation has reached max lifetime"
        );
    }

    should_reallocate
}

/// Group active allocations by their deployment ID.
fn group_allocations_by_deployment(allocations: &[ActiveAllocation]) -> AllocationsByDeployment {
    let mut map: AllocationsByDeployment = HashMap::new();

    for alloc in allocations {
        map.entry(alloc.deployment_id.clone())
            .or_default()
            .push(alloc.clone());
    }

    map
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bigdecimal::BigDecimal;
    use thegraph_core::alloy::primitives::Address;

    use super::*;
    use crate::rules::ActivationCriteria;

    fn make_decision(
        deployment_id: &str,
        to_allocate: bool,
        criteria: ActivationCriteria,
    ) -> AllocationDecision {
        AllocationDecision {
            deployment_id: deployment_id.to_string(),
            to_allocate,
            criteria,
            reason: format!("test: {:?}", criteria),
            rule: Some(crate::rules::MergedIndexingRule {
                identifier: "global".to_string(),
                identifier_type: crate::models::IdentifierType::Group,
                allocation_amount: Some(BigDecimal::from_str("1000").unwrap()),
                allocation_lifetime: Some(28),
                auto_renewal: true,
                parallel_allocations: None,
                max_allocation_percentage: None,
                min_signal: None,
                max_signal: None,
                min_stake: None,
                min_average_query_fees: None,
                custom: None,
                decision_basis: crate::models::IndexingDecisionBasis::Always,
                require_supported: true,
                safety: true,
                protocol_network: "eip155:1".to_string(),
            }),
        }
    }

    fn make_allocation(deployment_id: &str, created_epoch: u64) -> ActiveAllocation {
        ActiveAllocation {
            id: Address::ZERO,
            deployment_id: deployment_id.to_string(),
            allocated_tokens: "1000".to_string(),
            created_at_epoch: created_epoch,
            is_legacy: false,
        }
    }

    #[test]
    fn test_group_allocations_by_deployment() {
        let allocations = vec![
            make_allocation("dep1", 100),
            make_allocation("dep1", 101),
            make_allocation("dep2", 100),
        ];

        let grouped = group_allocations_by_deployment(&allocations);

        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get("dep1").unwrap().len(), 2);
        assert_eq!(grouped.get("dep2").unwrap().len(), 1);
    }

    #[test]
    fn test_should_reallocate_expired() {
        let current_epoch = 130;
        let max_allocation_epochs = 28;

        let allocation = make_allocation("dep1", 100); // Age = 30 epochs
        let decision = make_decision("dep1", true, ActivationCriteria::Always);

        // Allocation is 30 epochs old, lifetime is 28 -> should reallocate
        assert!(should_reallocate_with_params(
            current_epoch,
            max_allocation_epochs,
            &allocation,
            &decision
        ));
    }

    #[test]
    fn test_should_not_reallocate_fresh() {
        let current_epoch = 110;
        let max_allocation_epochs = 28;

        let allocation = make_allocation("dep1", 100); // Age = 10 epochs
        let decision = make_decision("dep1", true, ActivationCriteria::Always);

        // Allocation is 10 epochs old, lifetime is 28 -> should NOT reallocate
        assert!(!should_reallocate_with_params(
            current_epoch,
            max_allocation_epochs,
            &allocation,
            &decision
        ));
    }
}
