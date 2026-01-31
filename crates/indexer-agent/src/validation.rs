// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Input validation for agent operations.
//!
//! This module provides validation functions for identifiers and parameters
//! used in the indexer agent, including deployment IDs, protocol networks,
//! and allocation amounts.

use std::str::FromStr;

use alloy::primitives::U256;
use bigdecimal::{num_bigint::ToBigInt, BigDecimal, Signed};
use thegraph_core::DeploymentId;
use thiserror::Error;

/// Errors that can occur during input validation.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Invalid deployment ID format.
    #[error("invalid deployment ID '{0}': expected IPFS CIDv0 hash (Qm...) or bytes32 hex")]
    InvalidDeploymentId(String),

    /// Invalid protocol network format.
    #[error("invalid protocol network '{0}': expected CAIP-2 format (e.g., 'eip155:1')")]
    InvalidProtocolNetwork(String),

    /// Invalid allocation amount format.
    #[error("invalid allocation amount '{0}': {1}")]
    InvalidAmount(String, String),

    /// Zero allocation amount.
    #[error("allocation amount cannot be zero")]
    ZeroAmount,

    /// Allocation lifetime cannot be zero.
    #[error("allocation lifetime cannot be zero")]
    ZeroAllocationLifetime,

    /// Legacy actions are not supported.
    #[error("legacy actions are not supported; this agent only supports Horizon (V2) allocations")]
    LegacyActionNotSupported,
}

/// Validate a deployment identifier.
///
/// Accepts either:
/// - IPFS CIDv0 hash (starting with "Qm", 46 characters)
/// - Hex-encoded bytes32 (66 characters including "0x" prefix)
///
/// # Arguments
/// * `id` - The deployment identifier to validate
///
/// # Returns
/// * `Ok(())` if the identifier is valid
/// * `Err(ValidationError::InvalidDeploymentId)` if invalid
///
/// # Examples
/// ```
/// use indexer_agent::validation::validate_deployment_id;
///
/// // Valid IPFS hash
/// assert!(validate_deployment_id("QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY").is_ok());
///
/// // Valid bytes32 hex
/// assert!(validate_deployment_id("0x0000000000000000000000000000000000000000000000000000000000000001").is_ok());
///
/// // Invalid
/// assert!(validate_deployment_id("invalid").is_err());
/// ```
pub fn validate_deployment_id(id: &str) -> Result<(), ValidationError> {
    id.parse::<DeploymentId>()
        .map(|_| ())
        .map_err(|_| ValidationError::InvalidDeploymentId(id.to_string()))
}

/// Validate a protocol network identifier.
///
/// Expects CAIP-2 format: `namespace:reference`
///
/// Common valid values:
/// - `eip155:1` (Ethereum mainnet)
/// - `eip155:42161` (Arbitrum One)
/// - `eip155:421614` (Arbitrum Sepolia)
///
/// # Arguments
/// * `network` - The protocol network identifier to validate
///
/// # Returns
/// * `Ok(())` if the identifier is valid
/// * `Err(ValidationError::InvalidProtocolNetwork)` if invalid
///
/// # Examples
/// ```
/// use indexer_agent::validation::validate_protocol_network;
///
/// assert!(validate_protocol_network("eip155:1").is_ok());
/// assert!(validate_protocol_network("eip155:42161").is_ok());
/// assert!(validate_protocol_network("invalid").is_err());
/// ```
pub fn validate_protocol_network(network: &str) -> Result<(), ValidationError> {
    // CAIP-2 format: namespace:reference
    // namespace: [-a-z0-9]{3,8}
    // reference: [-a-zA-Z0-9]{1,32}
    let parts: Vec<&str> = network.split(':').collect();

    if parts.len() != 2 {
        return Err(ValidationError::InvalidProtocolNetwork(network.to_string()));
    }

    let namespace = parts[0];
    let reference = parts[1];

    // Validate namespace (3-8 lowercase alphanumeric + dash)
    if namespace.len() < 3
        || namespace.len() > 8
        || !namespace
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(ValidationError::InvalidProtocolNetwork(network.to_string()));
    }

    // Validate reference (1-32 alphanumeric + dash)
    if reference.is_empty()
        || reference.len() > 32
        || !reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ValidationError::InvalidProtocolNetwork(network.to_string()));
    }

    Ok(())
}

/// Validate an allocation amount.
///
/// Units policy (unambiguous):
/// - Integer strings are interpreted as wei (base-10) unless prefixed with 0x (hex wei)
/// - Decimal or scientific-notation strings are interpreted as GRT and converted to wei
///
/// The amount must be non-zero and representable as U256.
///
/// # Arguments
/// * `amount` - The allocation amount to validate
///
/// # Returns
/// * `Ok(())` if the amount is valid
/// * `Err(ValidationError::InvalidAmount)` if the format is invalid
/// * `Err(ValidationError::ZeroAmount)` if the amount is zero
///
/// # Examples
/// ```
/// use indexer_agent::validation::validate_allocation_amount;
///
/// // Valid amounts
/// assert!(validate_allocation_amount("1000000000000000000").is_ok());
/// assert!(validate_allocation_amount("0xde0b6b3a7640000").is_ok());
///
/// // Zero is invalid
/// assert!(validate_allocation_amount("0").is_err());
///
/// // Invalid format
/// assert!(validate_allocation_amount("not-a-number").is_err());
/// ```
pub fn validate_allocation_amount(amount: &str) -> Result<(), ValidationError> {
    let amount = amount.trim();

    // Empty string is invalid
    if amount.is_empty() {
        return Err(ValidationError::InvalidAmount(
            amount.to_string(),
            "amount cannot be empty".to_string(),
        ));
    }

    // Check hex prefix first (before decimal/scientific check, since hex can contain 'e')
    let value = if let Some(hex) = amount.strip_prefix("0x") {
        U256::from_str_radix(hex, 16)
            .map_err(|e| ValidationError::InvalidAmount(amount.to_string(), e.to_string()))?
    } else if amount.contains('.') || amount.contains('e') || amount.contains('E') {
        let grt = BigDecimal::from_str(amount)
            .map_err(|e| ValidationError::InvalidAmount(amount.to_string(), e.to_string()))?;

        if grt.is_negative() {
            return Err(ValidationError::InvalidAmount(
                amount.to_string(),
                "amount cannot be negative".to_string(),
            ));
        }

        let scale = BigDecimal::from_str("1000000000000000000").expect("valid scale");
        let wei = grt * scale;
        let wei_int = wei.to_bigint().ok_or_else(|| {
            ValidationError::InvalidAmount(
                amount.to_string(),
                "amount has fractional wei".to_string(),
            )
        })?;

        U256::from_str_radix(&wei_int.to_string(), 10)
            .map_err(|e| ValidationError::InvalidAmount(amount.to_string(), e.to_string()))?
    } else {
        U256::from_str_radix(amount, 10)
            .map_err(|e| ValidationError::InvalidAmount(amount.to_string(), e.to_string()))?
    };

    if value.is_zero() {
        return Err(ValidationError::ZeroAmount);
    }

    Ok(())
}

/// Validate an allocation lifetime.
///
/// The lifetime must be a positive integer representing epochs.
///
/// # Arguments
/// * `lifetime` - The allocation lifetime in epochs (if Some)
///
/// # Returns
/// * `Ok(())` if the lifetime is valid or None
/// * `Err(ValidationError::ZeroAllocationLifetime)` if lifetime is zero
pub fn validate_allocation_lifetime(lifetime: Option<u32>) -> Result<(), ValidationError> {
    if let Some(lt) = lifetime {
        if lt == 0 {
            return Err(ValidationError::ZeroAllocationLifetime);
        }
    }
    Ok(())
}

/// Validate that an action is not a legacy action.
///
/// This agent only supports Horizon (V2) allocations. Legacy actions
/// (from the V1 Staking contract) are rejected because the executor
/// only implements the Horizon SubgraphService contract path.
///
/// # Arguments
/// * `is_legacy` - Whether the action is marked as legacy
///
/// # Returns
/// * `Ok(())` if the action is not legacy (or is_legacy is None/false)
/// * `Err(ValidationError::LegacyActionNotSupported)` if is_legacy is true
///
/// # Examples
/// ```
/// use indexer_agent::validation::validate_not_legacy;
///
/// assert!(validate_not_legacy(None).is_ok());
/// assert!(validate_not_legacy(Some(false)).is_ok());
/// assert!(validate_not_legacy(Some(true)).is_err());
/// ```
pub fn validate_not_legacy(is_legacy: Option<bool>) -> Result<(), ValidationError> {
    if is_legacy == Some(true) {
        return Err(ValidationError::LegacyActionNotSupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_deployment_id_ipfs() {
        // Valid IPFS CIDv0 hashes
        assert!(validate_deployment_id("QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY").is_ok());
        assert!(validate_deployment_id("QmNLei78zWmzUdbeRB3CiUfAizWUrbeeZh5K1rhAQKCh51").is_ok());
    }

    #[test]
    fn test_validate_deployment_id_hex() {
        // Valid bytes32 hex
        assert!(validate_deployment_id(
            "0x0000000000000000000000000000000000000000000000000000000000000001"
        )
        .is_ok());
        assert!(validate_deployment_id(
            "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        )
        .is_ok());
    }

    #[test]
    fn test_validate_deployment_id_invalid() {
        // Too short
        assert!(matches!(
            validate_deployment_id("Qm123"),
            Err(ValidationError::InvalidDeploymentId(_))
        ));

        // Invalid prefix
        assert!(matches!(
            validate_deployment_id("invalid"),
            Err(ValidationError::InvalidDeploymentId(_))
        ));

        // Invalid hex (too short)
        assert!(matches!(
            validate_deployment_id("0x1234"),
            Err(ValidationError::InvalidDeploymentId(_))
        ));
    }

    #[test]
    fn test_validate_protocol_network_valid() {
        // Valid CAIP-2 networks
        assert!(validate_protocol_network("eip155:1").is_ok());
        assert!(validate_protocol_network("eip155:42161").is_ok());
        assert!(validate_protocol_network("eip155:421614").is_ok());
        assert!(validate_protocol_network("cosmos:cosmoshub-4").is_ok());
    }

    #[test]
    fn test_validate_protocol_network_invalid() {
        // Missing colon
        assert!(matches!(
            validate_protocol_network("eip1551"),
            Err(ValidationError::InvalidProtocolNetwork(_))
        ));

        // Empty reference
        assert!(matches!(
            validate_protocol_network("eip155:"),
            Err(ValidationError::InvalidProtocolNetwork(_))
        ));

        // Namespace too short
        assert!(matches!(
            validate_protocol_network("ei:1"),
            Err(ValidationError::InvalidProtocolNetwork(_))
        ));

        // Invalid characters in namespace
        assert!(matches!(
            validate_protocol_network("EIP155:1"),
            Err(ValidationError::InvalidProtocolNetwork(_))
        ));
    }

    #[test]
    fn test_validate_allocation_amount_valid() {
        // Decimal string
        assert!(validate_allocation_amount("1000000000000000000").is_ok());
        assert!(validate_allocation_amount("1").is_ok());
        assert!(validate_allocation_amount("100000000000000000000000000000000000000").is_ok());

        // Hex string
        assert!(validate_allocation_amount("0xde0b6b3a7640000").is_ok());
        assert!(validate_allocation_amount("0x1").is_ok());

        // Float notation
        assert!(validate_allocation_amount("1.5e18").is_ok());
        assert!(validate_allocation_amount("1000.0").is_ok());
    }

    #[test]
    fn test_validate_allocation_amount_zero() {
        assert!(matches!(
            validate_allocation_amount("0"),
            Err(ValidationError::ZeroAmount)
        ));
        assert!(matches!(
            validate_allocation_amount("0x0"),
            Err(ValidationError::ZeroAmount)
        ));
    }

    #[test]
    fn test_validate_allocation_amount_invalid() {
        assert!(matches!(
            validate_allocation_amount("not-a-number"),
            Err(ValidationError::InvalidAmount(_, _))
        ));
        assert!(matches!(
            validate_allocation_amount(""),
            Err(ValidationError::InvalidAmount(_, _))
        ));
    }

    #[test]
    fn test_validate_allocation_lifetime() {
        // Valid lifetimes
        assert!(validate_allocation_lifetime(None).is_ok());
        assert!(validate_allocation_lifetime(Some(1)).is_ok());
        assert!(validate_allocation_lifetime(Some(28)).is_ok());

        // Zero is invalid
        assert!(matches!(
            validate_allocation_lifetime(Some(0)),
            Err(ValidationError::ZeroAllocationLifetime)
        ));
    }

    #[test]
    fn test_validate_not_legacy() {
        // Non-legacy is valid
        assert!(validate_not_legacy(None).is_ok());
        assert!(validate_not_legacy(Some(false)).is_ok());

        // Legacy is rejected
        assert!(matches!(
            validate_not_legacy(Some(true)),
            Err(ValidationError::LegacyActionNotSupported)
        ));
    }
}
