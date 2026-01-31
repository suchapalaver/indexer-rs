// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Action executor for processing approved allocation actions.
//!
//! This module handles the execution of approved allocation actions
//! (allocate, unallocate, reallocate) by submitting transactions to
//! the Horizon (V2) contracts on-chain.
//!
//! Key components:
//! - [`ActionExecutor`] - Main executor that processes approved actions
//! - [`ProviderCache`] - Caches providers for nonce management
//! - [`contracts`] - Contract interfaces for SubgraphService and HorizonStaking

mod contracts;
mod errors;
mod provider;
mod runner;
mod transactions;

pub use contracts::{
    subgraph_service_eip712_domain, AllocationIdProof, HorizonStakingContract,
    SubgraphServiceContract,
};
pub use errors::{is_nonce_error, ExecutorError};
pub use provider::ProviderCache;
pub use runner::{ActionExecutor, ExecutorConfig};
pub use transactions::{
    build_allocate_tx, build_reallocate_tx, build_unallocate_tx, ReallocateParams,
};
