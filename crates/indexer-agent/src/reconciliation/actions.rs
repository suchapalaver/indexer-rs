// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Action queueing for allocation changes.

use sqlx::PgPool;
use tracing::{debug, info};

use crate::{
    metrics,
    models::{Action, ActionError, ActionInput, ActionType},
    rules::AllocationDecision,
};

/// Action to be queued for an allocation change.
#[derive(Debug, Clone)]
pub struct AllocationAction {
    /// Type of action (allocate, unallocate, reallocate)
    pub action_type: ActionType,
    /// Deployment ID
    pub deployment_id: String,
    /// Allocation ID (for unallocate/reallocate)
    pub allocation_id: Option<String>,
    /// Amount to allocate (for allocate/reallocate)
    pub amount: Option<String>,
    /// Reason for this action
    pub reason: String,
    /// Whether this is a legacy allocation
    pub is_legacy: bool,
}

/// Queue an allocation action for a deployment.
///
/// Creates a new allocation for a deployment that should be indexed
/// according to the rules evaluation.
///
/// This function enforces the action cooldown (Invariant 8.2/22.2) by checking
/// if an action was recently executed for this deployment before queueing a new one.
pub async fn queue_allocation_action(
    pool: &PgPool,
    protocol_network: &str,
    decision: &AllocationDecision,
    auto_approve: bool,
    cooldown_secs: u64,
) -> Result<Option<Action>, sqlx::Error> {
    let Some(ref rule) = decision.rule else {
        debug!(
            deployment = %decision.deployment_id,
            "No rule found for deployment, skipping allocation"
        );
        return Ok(None);
    };

    let Some(ref allocation_amount) = rule.allocation_amount else {
        debug!(
            deployment = %decision.deployment_id,
            "No allocation amount configured, skipping"
        );
        return Ok(None);
    };

    // Check if there's already a pending action for this deployment
    if has_pending_action(pool, protocol_network, &decision.deployment_id).await? {
        debug!(
            deployment = %decision.deployment_id,
            "Action already pending for deployment, skipping"
        );
        return Ok(None);
    }

    // Check if an action was recently executed (cooldown check - Invariant 8.2/22.2)
    if cooldown_secs > 0
        && Action::was_recently_executed(
            pool,
            &decision.deployment_id,
            protocol_network,
            cooldown_secs,
        )
        .await?
    {
        debug!(
            deployment = %decision.deployment_id,
            cooldown_secs = cooldown_secs,
            "Action recently executed for deployment, cooldown not expired"
        );
        metrics::record_action_cooldown_skip("allocate");
        return Ok(None);
    }

    let input = ActionInput {
        action_type: ActionType::Allocate,
        deployment_id: decision.deployment_id.clone(),
        allocation_id: None,
        amount: Some(allocation_amount.to_string()),
        poi: None,
        force: None,
        source: "indexerAgent".to_string(),
        reason: format!("Allocation decision: {}", decision.reason),
        priority: Some(0),
        protocol_network: protocol_network.to_string(),
        is_legacy: Some(false), // New allocations are always V2/Horizon
        public_poi: None,
        poi_block_number: None,
    };

    let action = match Action::queue(pool, input).await {
        Ok(action) => action,
        Err(ActionError::DuplicatePendingAction { deployment_id }) => {
            // Database constraint caught a race condition - another action was queued
            // between our pre-check and insert. This is expected and not an error.
            debug!(
                deployment = %deployment_id,
                "Action already pending for deployment (constraint), skipping"
            );
            return Ok(None);
        }
        Err(ActionError::Database(e)) => return Err(e),
    };

    // Auto-approve if in AUTO mode
    if auto_approve {
        let approved = Action::approve(pool, &[action.id], protocol_network).await?;
        if let Some(approved_action) = approved.into_iter().next() {
            info!(
                deployment = %decision.deployment_id,
                action_id = approved_action.id,
                amount = %allocation_amount,
                "Auto-approved allocation action"
            );
            return Ok(Some(approved_action));
        }
    }

    info!(
        deployment = %decision.deployment_id,
        action_id = action.id,
        amount = %allocation_amount,
        "Queued allocation action"
    );
    metrics::record_action_queued("allocate");

    Ok(Some(action))
}

/// Queue an unallocation action for an allocation that should be closed.
///
/// This function enforces the action cooldown (Invariant 8.2/22.2) by checking
/// if an action was recently executed for this deployment before queueing a new one.
#[allow(clippy::too_many_arguments)]
pub async fn queue_unallocation_action(
    pool: &PgPool,
    protocol_network: &str,
    deployment_id: &str,
    allocation_id: &str,
    reason: &str,
    is_legacy: bool,
    auto_approve: bool,
    cooldown_secs: u64,
) -> Result<Option<Action>, sqlx::Error> {
    // Check if there's already a pending action for this allocation
    if has_pending_action_for_allocation(pool, protocol_network, allocation_id).await? {
        debug!(
            allocation = %allocation_id,
            "Action already pending for allocation, skipping"
        );
        return Ok(None);
    }

    // Check if an action was recently executed (cooldown check - Invariant 8.2/22.2)
    if cooldown_secs > 0
        && Action::was_recently_executed(pool, deployment_id, protocol_network, cooldown_secs)
            .await?
    {
        debug!(
            deployment = %deployment_id,
            allocation = %allocation_id,
            cooldown_secs = cooldown_secs,
            "Action recently executed for deployment, cooldown not expired"
        );
        metrics::record_action_cooldown_skip("unallocate");
        return Ok(None);
    }

    let input = ActionInput {
        action_type: ActionType::Unallocate,
        deployment_id: deployment_id.to_string(),
        allocation_id: Some(allocation_id.to_string()),
        amount: None,
        poi: None,
        force: None,
        source: "indexerAgent".to_string(),
        reason: reason.to_string(),
        priority: Some(0),
        protocol_network: protocol_network.to_string(),
        is_legacy: Some(is_legacy),
        public_poi: None,
        poi_block_number: None,
    };

    let action = match Action::queue(pool, input).await {
        Ok(action) => action,
        Err(ActionError::DuplicatePendingAction { deployment_id }) => {
            debug!(
                deployment = %deployment_id,
                allocation = %allocation_id,
                "Action already pending for deployment (constraint), skipping"
            );
            return Ok(None);
        }
        Err(ActionError::Database(e)) => return Err(e),
    };

    // Auto-approve if in AUTO mode
    if auto_approve {
        let approved = Action::approve(pool, &[action.id], protocol_network).await?;
        if let Some(approved_action) = approved.into_iter().next() {
            info!(
                allocation = %allocation_id,
                deployment = %deployment_id,
                action_id = approved_action.id,
                "Auto-approved unallocation action"
            );
            return Ok(Some(approved_action));
        }
    }

    info!(
        allocation = %allocation_id,
        deployment = %deployment_id,
        action_id = action.id,
        "Queued unallocation action"
    );
    metrics::record_action_queued("unallocate");

    Ok(Some(action))
}

/// Queue a reallocation action for an expiring allocation.
///
/// This function enforces the action cooldown (Invariant 8.2/22.2) by checking
/// if an action was recently executed for this deployment before queueing a new one.
#[allow(clippy::too_many_arguments)]
pub async fn queue_reallocation_action(
    pool: &PgPool,
    protocol_network: &str,
    deployment_id: &str,
    allocation_id: &str,
    amount: &str,
    is_legacy: bool,
    auto_approve: bool,
    cooldown_secs: u64,
) -> Result<Option<Action>, sqlx::Error> {
    // Check if there's already a pending action for this allocation
    if has_pending_action_for_allocation(pool, protocol_network, allocation_id).await? {
        debug!(
            allocation = %allocation_id,
            "Action already pending for allocation, skipping"
        );
        return Ok(None);
    }

    // Check if an action was recently executed (cooldown check - Invariant 8.2/22.2)
    if cooldown_secs > 0
        && Action::was_recently_executed(pool, deployment_id, protocol_network, cooldown_secs)
            .await?
    {
        debug!(
            deployment = %deployment_id,
            allocation = %allocation_id,
            cooldown_secs = cooldown_secs,
            "Action recently executed for deployment, cooldown not expired"
        );
        metrics::record_action_cooldown_skip("reallocate");
        return Ok(None);
    }

    let input = ActionInput {
        action_type: ActionType::Reallocate,
        deployment_id: deployment_id.to_string(),
        allocation_id: Some(allocation_id.to_string()),
        amount: Some(amount.to_string()),
        poi: None,
        force: None,
        source: "indexerAgent".to_string(),
        reason: "Allocation approaching max lifetime, refreshing".to_string(),
        priority: Some(0),
        protocol_network: protocol_network.to_string(),
        is_legacy: Some(is_legacy),
        public_poi: None,
        poi_block_number: None,
    };

    let action = match Action::queue(pool, input).await {
        Ok(action) => action,
        Err(ActionError::DuplicatePendingAction { deployment_id }) => {
            debug!(
                deployment = %deployment_id,
                allocation = %allocation_id,
                "Action already pending for deployment (constraint), skipping"
            );
            return Ok(None);
        }
        Err(ActionError::Database(e)) => return Err(e),
    };

    // Auto-approve if in AUTO mode
    if auto_approve {
        let approved = Action::approve(pool, &[action.id], protocol_network).await?;
        if let Some(approved_action) = approved.into_iter().next() {
            info!(
                allocation = %allocation_id,
                deployment = %deployment_id,
                action_id = approved_action.id,
                "Auto-approved reallocation action"
            );
            return Ok(Some(approved_action));
        }
    }

    info!(
        allocation = %allocation_id,
        deployment = %deployment_id,
        action_id = action.id,
        "Queued reallocation action"
    );
    metrics::record_action_queued("reallocate");

    Ok(Some(action))
}

/// Check if there's already a pending (queued/approved) action for a deployment.
async fn has_pending_action(
    pool: &PgPool,
    protocol_network: &str,
    deployment_id: &str,
) -> Result<bool, sqlx::Error> {
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM "Actions"
        WHERE deployment_id = $1
          AND protocol_network = $2
          AND status IN ('queued', 'approved', 'pending')
        "#,
    )
    .bind(deployment_id)
    .bind(protocol_network)
    .fetch_one(pool)
    .await?;

    Ok(count.0 > 0)
}

/// Check if there's already a pending action for a specific allocation.
async fn has_pending_action_for_allocation(
    pool: &PgPool,
    protocol_network: &str,
    allocation_id: &str,
) -> Result<bool, sqlx::Error> {
    let count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM "Actions"
        WHERE allocation_id = $1
          AND protocol_network = $2
          AND status IN ('queued', 'approved', 'pending')
        "#,
    )
    .bind(allocation_id)
    .bind(protocol_network)
    .fetch_one(pool)
    .await?;

    Ok(count.0 > 0)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bigdecimal::BigDecimal;

    use super::*;
    use crate::rules::{ActivationCriteria, MergedIndexingRule};

    fn make_decision(deployment_id: &str, to_allocate: bool) -> AllocationDecision {
        AllocationDecision {
            deployment_id: deployment_id.to_string(),
            to_allocate,
            criteria: if to_allocate {
                ActivationCriteria::Always
            } else {
                ActivationCriteria::Never
            },
            reason: "test".to_string(),
            rule: Some(MergedIndexingRule {
                identifier: "global".to_string(),
                identifier_type: crate::models::IdentifierType::Group,
                allocation_amount: Some(BigDecimal::from_str("1000000000000000000000").unwrap()),
                allocation_lifetime: None,
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

    #[test]
    fn test_allocation_action_creation() {
        let decision = make_decision("Qm123", true);
        assert!(decision.rule.is_some());
        assert!(decision.rule.unwrap().allocation_amount.is_some());
    }
}
