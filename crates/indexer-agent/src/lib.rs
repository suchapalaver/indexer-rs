// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Indexer agent functionality for the unified indexer-rs binary.
//!
//! This crate provides:
//! - Data models for indexing rules, actions, and POI disputes
//! - Database operations for the agent tables
//! - Rules evaluation engine for allocation decisions
//! - Business logic for allocation management

pub mod models;
pub mod rules;

pub use models::{
    Action, ActionFilter, ActionInput, ActionStatus, ActionType, IdentifierType,
    IndexingDecisionBasis, IndexingRule, IndexingRuleInput, POIDispute, POIDisputeInput,
};
pub use rules::{
    evaluate_deployments, is_deployment_worth_allocating, ActivationCriteria, AllocationDecision,
    MergedIndexingRule, NetworkDeployment, PreprocessedRules,
};
