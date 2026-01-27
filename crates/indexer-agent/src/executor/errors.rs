// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Error types for the action executor.

use alloy::primitives::Address;
use thiserror::Error;

/// Errors that can occur during action execution.
#[derive(Error, Debug)]
pub enum ExecutorError {
    /// Action not found in database
    #[error("action not found: {action_id}")]
    ActionNotFound { action_id: i32 },

    /// Operator not authorized for indexer
    #[error("operator {operator} is not authorized for indexer {indexer}")]
    UnauthorizedOperator { indexer: Address, operator: Address },

    /// Network is paused, no transactions allowed
    #[error("network is paused, cannot submit transactions")]
    NetworkPaused,

    /// Action is not in approved state
    #[error("action {action_id} is not approved, status: {status}")]
    ActionNotApproved { action_id: i32, status: String },

    /// Missing required field for action
    #[error("missing required field '{field}' for action {action_id}")]
    MissingField { action_id: i32, field: String },

    /// Invalid allocation amount
    #[error("invalid allocation amount '{amount}' for action {action_id}: {reason}")]
    InvalidAmount {
        action_id: i32,
        amount: String,
        reason: String,
    },

    /// Provider error
    #[error("provider error: {0}")]
    Provider(String),

    /// Transaction building error
    #[error("failed to build transaction: {0}")]
    TransactionBuild(String),

    /// Transaction submission failed
    #[error("transaction submission failed: {0}")]
    TransactionSubmit(String),

    /// Transaction reverted on-chain
    #[error("transaction reverted: {tx_hash}")]
    TransactionReverted { tx_hash: String },

    /// Transaction not found after submission
    #[error("transaction not found after {retries} retries: {tx_hash}")]
    TransactionNotFound { tx_hash: String, retries: u32 },

    /// Transaction still pending after timeout
    #[error("transaction still pending after timeout: {tx_hash}")]
    TransactionPending { tx_hash: String },

    /// Gas estimation failed
    #[error("gas estimation failed: {0}")]
    GasEstimation(String),

    /// Insufficient balance for gas
    #[error("insufficient balance for gas: have {available}, need {required}")]
    InsufficientGasBalance { available: String, required: String },

    /// Contract call error
    #[error("contract call failed: {0}")]
    ContractCall(String),

    /// Database error
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Configuration error
    #[error("configuration error: {0}")]
    Config(String),

    /// Signing error
    #[error("signing error: {0}")]
    Signing(String),

    /// Allocation proof generation failed
    #[error("failed to generate allocation proof: {0}")]
    AllocationProof(String),
}

/// Patterns that indicate a nonce-related error.
const NONCE_ERROR_PATTERNS: &[&str] = &[
    "nonce too low",
    "nonce too high",
    "invalid nonce",
    "nonce has already been used",
    "replacement transaction underpriced",
    "already known",
    "transaction with same nonce",
];

/// Check if an error message indicates a nonce-related issue.
///
/// These errors typically require resetting the provider cache to
/// fetch a fresh nonce from the network.
pub fn is_nonce_error(error_message: &str) -> bool {
    let lower = error_message.to_lowercase();
    NONCE_ERROR_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonce_error_detection() {
        assert!(is_nonce_error("nonce too low"));
        assert!(is_nonce_error("Error: Nonce Too Low for account"));
        assert!(is_nonce_error("replacement transaction underpriced"));
        assert!(is_nonce_error("transaction with same nonce already exists"));
        assert!(is_nonce_error("NONCE HAS ALREADY BEEN USED"));

        assert!(!is_nonce_error("insufficient funds"));
        assert!(!is_nonce_error("gas limit exceeded"));
        assert!(!is_nonce_error("contract reverted"));
    }

    #[test]
    fn test_unauthorized_operator_error() {
        let indexer = Address::repeat_byte(0x11);
        let operator = Address::repeat_byte(0x22);
        let error = ExecutorError::UnauthorizedOperator { indexer, operator };

        let message = error.to_string();
        assert!(message.contains("0x1111111111111111111111111111111111111111"));
        assert!(message.contains("0x2222222222222222222222222222222222222222"));
        assert!(message.contains("not authorized"));
    }

    #[test]
    fn test_network_paused_error() {
        let error = ExecutorError::NetworkPaused;
        let message = error.to_string();
        assert!(message.contains("network is paused"));
        assert!(message.contains("cannot submit transactions"));
    }
}
