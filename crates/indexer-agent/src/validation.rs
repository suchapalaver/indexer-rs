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

    /// Missing required field for action type.
    #[error("{action_type} action requires {field}")]
    MissingRequiredField {
        action_type: &'static str,
        field: &'static str,
    },

    /// Invalid allocation ID format.
    #[error("invalid allocation ID '{0}': expected 0x-prefixed 40-character hex address")]
    InvalidAllocationId(String),
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

/// Validate an allocation ID.
///
/// Expects a 0x-prefixed 40-character hexadecimal address.
///
/// # Arguments
/// * `allocation_id` - The allocation ID to validate
///
/// # Returns
/// * `Ok(())` if the allocation ID is valid
/// * `Err(ValidationError::InvalidAllocationId)` if invalid
///
/// # Examples
/// ```
/// use indexer_agent::validation::validate_allocation_id;
///
/// assert!(validate_allocation_id("0x1234567890123456789012345678901234567890").is_ok());
/// assert!(validate_allocation_id("invalid").is_err());
/// ```
pub fn validate_allocation_id(allocation_id: &str) -> Result<(), ValidationError> {
    let allocation_id = allocation_id.trim();

    // Must start with 0x
    let hex = allocation_id
        .strip_prefix("0x")
        .or_else(|| allocation_id.strip_prefix("0X"))
        .ok_or_else(|| ValidationError::InvalidAllocationId(allocation_id.to_string()))?;

    // Must be exactly 40 hex characters (20 bytes)
    if hex.len() != 40 {
        return Err(ValidationError::InvalidAllocationId(
            allocation_id.to_string(),
        ));
    }

    // Must be valid hex
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError::InvalidAllocationId(
            allocation_id.to_string(),
        ));
    }

    Ok(())
}

/// Validate an action input before queueing.
///
/// This performs comprehensive validation of action inputs at the API boundary,
/// ensuring invalid actions are rejected before reaching the database. Validation
/// includes:
///
/// - Deployment ID format (IPFS CIDv0 or bytes32 hex)
/// - Protocol network format (CAIP-2)
/// - Legacy action rejection (only Horizon/V2 supported)
/// - Action type-specific required fields:
///   - `allocate`: requires `amount`
///   - `unallocate`: requires `allocation_id`
///   - `reallocate`: requires both `allocation_id` and `amount`
/// - Field format validation (allocation_id, amount)
///
/// # Arguments
/// * `action_type` - The type of action (allocate, unallocate, reallocate)
/// * `deployment_id` - The deployment identifier
/// * `protocol_network` - The protocol network identifier
/// * `allocation_id` - The allocation ID (required for unallocate/reallocate)
/// * `amount` - The allocation amount (required for allocate/reallocate)
/// * `is_legacy` - Whether this is a legacy action (must be false or None)
///
/// # Returns
/// * `Ok(())` if all validations pass
/// * `Err(ValidationError)` describing the first validation failure
pub fn validate_action_input(
    action_type: &str,
    deployment_id: &str,
    protocol_network: &str,
    allocation_id: Option<&str>,
    amount: Option<&str>,
    is_legacy: Option<bool>,
) -> Result<(), ValidationError> {
    // Validate deployment ID format
    validate_deployment_id(deployment_id)?;

    // Validate protocol network format
    validate_protocol_network(protocol_network)?;

    // Reject legacy actions
    validate_not_legacy(is_legacy)?;

    // Validate action type-specific requirements
    match action_type {
        "allocate" => {
            // Allocate requires amount
            let amt = amount.ok_or(ValidationError::MissingRequiredField {
                action_type: "allocate",
                field: "amount",
            })?;
            validate_allocation_amount(amt)?;
        }
        "unallocate" => {
            // Unallocate requires allocation_id
            let alloc_id = allocation_id.ok_or(ValidationError::MissingRequiredField {
                action_type: "unallocate",
                field: "allocation_id",
            })?;
            validate_allocation_id(alloc_id)?;
        }
        "reallocate" => {
            // Reallocate requires both allocation_id and amount
            let alloc_id = allocation_id.ok_or(ValidationError::MissingRequiredField {
                action_type: "reallocate",
                field: "allocation_id",
            })?;
            validate_allocation_id(alloc_id)?;

            let amt = amount.ok_or(ValidationError::MissingRequiredField {
                action_type: "reallocate",
                field: "amount",
            })?;
            validate_allocation_amount(amt)?;
        }
        _ => {
            // Unknown action type - let database constraint handle it
        }
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

    #[test]
    fn test_validate_allocation_id_valid() {
        // Valid allocation IDs
        assert!(validate_allocation_id("0x1234567890123456789012345678901234567890").is_ok());
        assert!(validate_allocation_id("0xabcdef0123456789abcdef0123456789abcdef01").is_ok());
        assert!(validate_allocation_id("0xABCDEF0123456789ABCDEF0123456789ABCDEF01").is_ok());
        // With whitespace
        assert!(validate_allocation_id("  0x1234567890123456789012345678901234567890  ").is_ok());
    }

    #[test]
    fn test_validate_allocation_id_invalid() {
        // Missing 0x prefix
        assert!(matches!(
            validate_allocation_id("1234567890123456789012345678901234567890"),
            Err(ValidationError::InvalidAllocationId(_))
        ));

        // Too short
        assert!(matches!(
            validate_allocation_id("0x123456"),
            Err(ValidationError::InvalidAllocationId(_))
        ));

        // Too long
        assert!(matches!(
            validate_allocation_id("0x12345678901234567890123456789012345678901234"),
            Err(ValidationError::InvalidAllocationId(_))
        ));

        // Invalid hex characters
        assert!(matches!(
            validate_allocation_id("0x123456789012345678901234567890123456789g"),
            Err(ValidationError::InvalidAllocationId(_))
        ));
    }

    #[test]
    fn test_validate_action_input_allocate() {
        let deployment = "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY";
        let network = "eip155:42161";

        // Valid allocate action
        assert!(validate_action_input(
            "allocate",
            deployment,
            network,
            None,
            Some("1000000000000000000"),
            None
        )
        .is_ok());

        // Allocate missing amount
        assert!(matches!(
            validate_action_input("allocate", deployment, network, None, None, None),
            Err(ValidationError::MissingRequiredField {
                action_type: "allocate",
                field: "amount"
            })
        ));

        // Allocate with invalid amount
        assert!(matches!(
            validate_action_input(
                "allocate",
                deployment,
                network,
                None,
                Some("not-a-number"),
                None
            ),
            Err(ValidationError::InvalidAmount(_, _))
        ));

        // Allocate with zero amount
        assert!(matches!(
            validate_action_input("allocate", deployment, network, None, Some("0"), None),
            Err(ValidationError::ZeroAmount)
        ));
    }

    #[test]
    fn test_validate_action_input_unallocate() {
        let deployment = "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY";
        let network = "eip155:42161";
        let allocation_id = "0x1234567890123456789012345678901234567890";

        // Valid unallocate action
        assert!(validate_action_input(
            "unallocate",
            deployment,
            network,
            Some(allocation_id),
            None,
            None
        )
        .is_ok());

        // Unallocate missing allocation_id
        assert!(matches!(
            validate_action_input("unallocate", deployment, network, None, None, None),
            Err(ValidationError::MissingRequiredField {
                action_type: "unallocate",
                field: "allocation_id"
            })
        ));

        // Unallocate with invalid allocation_id
        assert!(matches!(
            validate_action_input(
                "unallocate",
                deployment,
                network,
                Some("invalid"),
                None,
                None
            ),
            Err(ValidationError::InvalidAllocationId(_))
        ));
    }

    #[test]
    fn test_validate_action_input_reallocate() {
        let deployment = "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY";
        let network = "eip155:42161";
        let allocation_id = "0x1234567890123456789012345678901234567890";

        // Valid reallocate action
        assert!(validate_action_input(
            "reallocate",
            deployment,
            network,
            Some(allocation_id),
            Some("1000000000000000000"),
            None
        )
        .is_ok());

        // Reallocate missing allocation_id
        assert!(matches!(
            validate_action_input(
                "reallocate",
                deployment,
                network,
                None,
                Some("1000000000000000000"),
                None
            ),
            Err(ValidationError::MissingRequiredField {
                action_type: "reallocate",
                field: "allocation_id"
            })
        ));

        // Reallocate missing amount
        assert!(matches!(
            validate_action_input(
                "reallocate",
                deployment,
                network,
                Some(allocation_id),
                None,
                None
            ),
            Err(ValidationError::MissingRequiredField {
                action_type: "reallocate",
                field: "amount"
            })
        ));
    }

    #[test]
    fn test_validate_action_input_common_validations() {
        let allocation_id = "0x1234567890123456789012345678901234567890";

        // Invalid deployment ID
        assert!(matches!(
            validate_action_input(
                "allocate",
                "invalid",
                "eip155:42161",
                None,
                Some("1000000000000000000"),
                None
            ),
            Err(ValidationError::InvalidDeploymentId(_))
        ));

        // Invalid protocol network
        assert!(matches!(
            validate_action_input(
                "allocate",
                "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY",
                "invalid",
                None,
                Some("1000000000000000000"),
                None
            ),
            Err(ValidationError::InvalidProtocolNetwork(_))
        ));

        // Legacy action rejected
        assert!(matches!(
            validate_action_input(
                "unallocate",
                "QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY",
                "eip155:42161",
                Some(allocation_id),
                None,
                Some(true)
            ),
            Err(ValidationError::LegacyActionNotSupported)
        ));
    }
}
