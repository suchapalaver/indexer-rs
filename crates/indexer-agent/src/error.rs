// Copyright 2026-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Error taxonomy for the indexer-agent crate.
//!
//! This provides a lightweight classification layer so callers can reason about
//! error categories without changing error messages (logs remain stable).

/// High-level error classes for operational decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Invalid or missing user input.
    Input,
    /// Violation of internal business or state invariants.
    Invariant,
    /// External dependency failure (db/network/chain).
    External,
    /// Internal bug or misconfiguration.
    Internal,
}

/// Trait for classifying errors without changing their Display output.
pub trait ErrorClassification {
    fn class(&self) -> ErrorClass;
}
