// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Indexing rules evaluation and merging logic.
//!
//! This module provides functionality for:
//! - Evaluating whether deployments are worth allocating towards
//! - Merging specific rules with global defaults
//! - Preprocessing rules for efficient O(1) lookup

mod evaluation;
mod merging;

pub use evaluation::{
    evaluate_deployments, is_deployment_worth_allocating, ActivationCriteria, AllocationDecision,
    NetworkDeployment, PreprocessedRules,
};
pub use merging::MergedIndexingRule;
