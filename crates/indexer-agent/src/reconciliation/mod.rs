// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Reconciliation loop for synchronizing indexer state with the network.
//!
//! The reconciliation loop periodically:
//! 1. Fetches deployment data from the network subgraph
//! 2. Evaluates deployments against indexing rules
//! 3. Compares desired state against current allocations
//! 4. Queues actions for changes needed (allocate/unallocate/reallocate)

mod actions;
mod context;
mod deployments;
mod loop_task;

pub use actions::{queue_allocation_action, queue_unallocation_action, AllocationAction};
pub use context::{ActiveAllocation, NetworkDeploymentData, ReconciliationContext};
pub use deployments::{reconcile_deployment_allocations, DeploymentReconciliation};
pub use loop_task::{reconcile_once, run_reconciliation_loop, ReconciliationConfig};
