// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Indexer agent functionality for the unified indexer-rs binary.
//!
//! This crate provides:
//! - Data models for indexing rules, actions, and POI disputes
//! - Database operations for the agent tables
//! - Rules evaluation engine for allocation decisions
//! - Reconciliation loop for allocation management
//! - Action executor for on-chain transaction execution
//! - Business logic for allocation management
//! - Input validation for identifiers and parameters

pub mod executor;
pub mod models;
pub mod reconciliation;
pub mod rules;
pub mod validation;

pub use executor::{
    is_nonce_error, ActionExecutor, ExecutorConfig, ExecutorError, HorizonStakingContract,
    ProviderCache, SubgraphServiceContract,
};
pub use models::{
    Action, ActionError, ActionFilter, ActionInput, ActionStatus, ActionType, IdentifierType,
    IndexingDecisionBasis, IndexingRule, IndexingRuleInput, POIDispute, POIDisputeInput,
};
pub use reconciliation::{
    queue_allocation_action, queue_unallocation_action, reconcile_deployment_allocations,
    reconcile_once, run_reconciliation_loop, ActiveAllocation, AllocationAction,
    DeploymentReconciliation, NetworkDeploymentData, ReconciliationConfig, ReconciliationContext,
};
pub use rules::{
    evaluate_deployments, is_deployment_worth_allocating, ActivationCriteria, AllocationDecision,
    MergedIndexingRule, NetworkDeployment, PreprocessedRules,
};
pub use validation::{
    validate_allocation_amount, validate_allocation_lifetime, validate_deployment_id,
    validate_protocol_network, ValidationError,
};
