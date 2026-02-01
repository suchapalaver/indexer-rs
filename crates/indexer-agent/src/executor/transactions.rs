// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Transaction builders for allocation actions.
//!
//! This module provides functions to build transactions for:
//! - Creating allocations (startService)
//! - Closing allocations (collect + stopService)
//! - Reallocating (close existing + open new)

use alloy::{
    primitives::{Address, BlockNumber, Bytes, FixedBytes, U256},
    sol,
    sol_types::{SolCall, SolValue},
};

use super::{
    contracts::{PaymentType, SubgraphService},
    errors::ExecutorError,
};
use crate::amounts::parse_amount_wei;

/// Gas buffer multiplier for allocation transactions.
/// We add 30% to the estimated gas to account for state changes.
pub const GAS_BUFFER_PERCENT: u64 = 130;

// Define types for ABI encoding using sol! macro
sol! {
    /// StartService data structure for encoding
    struct StartServiceData {
        bytes32 subgraphDeploymentId;
        uint256 tokens;
        address allocationId;
        bytes proof;
    }

    /// CollectIndexingRewards data structure
    struct CollectIndexingRewardsData {
        address allocationId;
        bytes32 poi;
        bytes metadata;
    }

    /// Proof of Indexing (POI) metadata structure
    struct POIMetadata {
        uint256 blockNumber;
        bytes32 publicPoi;
        string indexingStatus;
        uint256 reserved1;
        uint256 reserved2;
    }

    /// StopService data structure
    struct StopServiceData {
        address allocationId;
    }
}

/// Encode data for starting service (creating an allocation).
///
/// # Arguments
/// * `subgraph_deployment_id` - The deployment ID (bytes32)
/// * `tokens` - Amount of GRT to allocate (in wei)
/// * `allocation_id` - The unique allocation ID
/// * `proof` - Allocation ID proof signature
pub fn encode_start_service_data(
    subgraph_deployment_id: FixedBytes<32>,
    tokens: U256,
    allocation_id: Address,
    proof: Bytes,
) -> Bytes {
    let data = StartServiceData {
        subgraphDeploymentId: subgraph_deployment_id,
        tokens,
        allocationId: allocation_id,
        proof,
    };
    data.abi_encode().into()
}

/// Encode data for collecting indexing rewards.
///
/// # Arguments
/// * `allocation_id` - The allocation ID
/// * `poi` - Proof of Indexing (bytes32)
/// * `metadata` - POI metadata (encoded)
pub fn encode_collect_indexing_rewards_data(
    allocation_id: Address,
    poi: FixedBytes<32>,
    metadata: Bytes,
) -> Bytes {
    let data = CollectIndexingRewardsData {
        allocationId: allocation_id,
        poi,
        metadata,
    };
    data.abi_encode().into()
}

/// Encode Proof of Indexing (POI) metadata.
///
/// # Arguments
/// * `block_number` - The block number for the POI
/// * `public_poi` - Optional public POI (bytes32)
/// * `indexing_status` - Optional indexing status string
pub fn encode_poi_metadata(
    block_number: BlockNumber,
    public_poi: Option<FixedBytes<32>>,
    indexing_status: Option<&str>,
) -> Bytes {
    let data = POIMetadata {
        blockNumber: U256::from(block_number),
        publicPoi: public_poi.unwrap_or_default(),
        indexingStatus: indexing_status.unwrap_or_default().to_string(),
        reserved1: U256::ZERO,
        reserved2: U256::ZERO,
    };
    data.abi_encode().into()
}

/// Encode data for stopping service (closing an allocation).
///
/// # Arguments
/// * `allocation_id` - The allocation ID to close
pub fn encode_stop_service_data(allocation_id: Address) -> Bytes {
    let data = StopServiceData {
        allocationId: allocation_id,
    };
    data.abi_encode().into()
}

/// Build transaction data for creating an allocation.
///
/// # Arguments
/// * `indexer` - The indexer's address
/// * `subgraph_deployment_id` - The deployment ID
/// * `tokens` - Amount of GRT to allocate (in wei)
/// * `allocation_id` - The unique allocation ID
/// * `proof` - Allocation ID proof signature
///
/// # Returns
/// The encoded calldata for SubgraphService.startService
pub fn build_allocate_tx(
    indexer: Address,
    subgraph_deployment_id: FixedBytes<32>,
    tokens: U256,
    allocation_id: Address,
    proof: Bytes,
) -> Bytes {
    let data = encode_start_service_data(subgraph_deployment_id, tokens, allocation_id, proof);
    let call = SubgraphService::startServiceCall { indexer, data };
    call.abi_encode().into()
}

/// Build transaction data for closing an allocation (unallocate).
///
/// For Horizon V2, closing an allocation requires:
/// 1. Calling collect() with indexing rewards
/// 2. If not over-allocated, calling stopService()
///
/// This function returns the calldata for a multicall that combines both.
///
/// # Arguments
/// * `indexer` - The indexer's address
/// * `allocation_id` - The allocation ID to close
/// * `poi` - Proof of Indexing (or zero bytes32 to force close)
/// * `poi_block_number` - The block number for the POI
/// * `public_poi` - Optional public POI
/// * `is_over_allocated` - Whether the indexer is over-allocated
///
/// # Returns
/// The encoded calldata for SubgraphService.multicall or collect
pub fn build_unallocate_tx(
    indexer: Address,
    allocation_id: Address,
    poi: FixedBytes<32>,
    poi_block_number: BlockNumber,
    public_poi: Option<FixedBytes<32>>,
    is_over_allocated: bool,
) -> Bytes {
    let metadata = encode_poi_metadata(poi_block_number, public_poi, None);
    let collect_data = encode_collect_indexing_rewards_data(allocation_id, poi, metadata);

    if is_over_allocated {
        // If over-allocated, collect() alone will close the allocation
        let call = SubgraphService::collectCall {
            indexer,
            paymentType: PaymentType::IndexingRewards.into(),
            data: collect_data,
        };
        call.abi_encode().into()
    } else {
        // Otherwise, we need to multicall collect() + stopService()
        let collect_call = SubgraphService::collectCall {
            indexer,
            paymentType: PaymentType::IndexingRewards.into(),
            data: collect_data,
        };

        let stop_data = encode_stop_service_data(allocation_id);
        let stop_call = SubgraphService::stopServiceCall {
            indexer,
            data: stop_data,
        };

        let multicall = SubgraphService::multicallCall {
            data: vec![
                collect_call.abi_encode().into(),
                stop_call.abi_encode().into(),
            ],
        };

        multicall.abi_encode().into()
    }
}

/// Parameters for building a reallocate transaction.
#[derive(Debug, Clone)]
pub struct ReallocateParams {
    /// The indexer's address
    pub indexer: Address,
    /// The allocation ID to close
    pub old_allocation_id: Address,
    /// Proof of Indexing for the old allocation
    pub poi: FixedBytes<32>,
    /// The block number for the Proof of Indexing (POI)
    pub poi_block_number: BlockNumber,
    /// The deployment ID (same as old allocation)
    pub subgraph_deployment_id: FixedBytes<32>,
    /// Amount of GRT for the new allocation
    pub new_tokens: U256,
    /// The new allocation ID
    pub new_allocation_id: Address,
    /// Allocation ID proof for the new allocation
    pub new_proof: Bytes,
    /// Whether the indexer is over-allocated
    pub is_over_allocated: bool,
}

/// Build transaction data for reallocating (close + open new allocation).
///
/// # Arguments
/// * `params` - Parameters for the reallocate transaction
///
/// # Returns
/// A tuple of (close_tx, allocate_tx) calldata
pub fn build_reallocate_tx(params: ReallocateParams) -> (Bytes, Bytes) {
    // Build close transaction
    let close_tx = build_unallocate_tx(
        params.indexer,
        params.old_allocation_id,
        params.poi,
        params.poi_block_number,
        None,
        params.is_over_allocated,
    );

    // Build new allocation transaction
    let allocate_tx = build_allocate_tx(
        params.indexer,
        params.subgraph_deployment_id,
        params.new_tokens,
        params.new_allocation_id,
        params.new_proof,
    );

    (close_tx, allocate_tx)
}

/// Parse an amount string to U256 wei value.
///
/// Units policy (unambiguous):
/// - Integer strings are interpreted as wei (base-10) unless prefixed with 0x (hex wei)
/// - Decimal or scientific-notation strings are interpreted as GRT and converted to wei
///
/// # Arguments
/// * `amount` - The amount string (e.g., "1000000000000000000", "0xde0b6b3a7640000", "1.5")
///
/// # Returns
/// The amount in wei as U256
pub fn parse_amount(amount: &str) -> Result<U256, ExecutorError> {
    parse_amount_wei(amount).map_err(|e| ExecutorError::InvalidAmount {
        action_id: 0,
        amount: amount.trim().to_string(),
        reason: e.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_amount_grt() {
        // 100 GRT = 100 * 10^18 wei (use decimal to indicate GRT)
        let amount = parse_amount("100.0").unwrap();
        assert_eq!(amount, U256::from(100_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_amount_decimal() {
        // 100.5 GRT
        let amount = parse_amount("100.5").unwrap();
        assert_eq!(amount, U256::from(100_500_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_amount_wei_integer() {
        // Treat integer as wei
        let amount = parse_amount("1000000000000000000").unwrap();
        assert_eq!(amount, U256::from(1_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_parse_amount_large_wei() {
        // Large wei value beyond u128
        let amount_str = "100000000000000000000000000000000000000";
        let amount = parse_amount(amount_str).unwrap();
        let expected = U256::from_str_radix(amount_str, 10).unwrap();
        assert_eq!(amount, expected);
    }

    #[test]
    fn test_encode_stop_service_data() {
        let allocation_id = Address::ZERO;
        let data = encode_stop_service_data(allocation_id);
        assert!(!data.is_empty());
    }

    #[test]
    fn test_build_allocate_tx() {
        let indexer = Address::ZERO;
        let deployment_id = FixedBytes::ZERO;
        let tokens = U256::from(1000000000000000000u128);
        let allocation_id = Address::ZERO;
        let proof = Bytes::new();

        let tx = build_allocate_tx(indexer, deployment_id, tokens, allocation_id, proof);
        assert!(!tx.is_empty());
    }

    #[test]
    fn test_build_unallocate_tx_over_allocated() {
        let indexer = Address::ZERO;
        let allocation_id = Address::ZERO;
        let poi = FixedBytes::ZERO;

        let tx = build_unallocate_tx(
            indexer,
            allocation_id,
            poi,
            BlockNumber::from(0u64),
            None,
            true,
        );
        assert!(!tx.is_empty());
    }

    #[test]
    fn test_build_unallocate_tx_normal() {
        let indexer = Address::ZERO;
        let allocation_id = Address::ZERO;
        let poi = FixedBytes::ZERO;

        let tx = build_unallocate_tx(
            indexer,
            allocation_id,
            poi,
            BlockNumber::from(0u64),
            None,
            false,
        );
        // Should be a multicall with collect + stopService
        assert!(!tx.is_empty());
    }
}
