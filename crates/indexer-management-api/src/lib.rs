// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! GraphQL Management API for indexer-rs
//!
//! This crate provides the management API that allows operators to:
//! - Manage indexing rules
//! - Queue and manage allocation actions
//! - Handle POI disputes

mod resolvers;
mod schema;

pub use schema::{build_schema, ManagementSchema};
