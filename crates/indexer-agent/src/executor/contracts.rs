// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Contract interfaces for Horizon (V2) allocation management.
//!
//! This module defines the Solidity interfaces for interacting with
//! the SubgraphService and HorizonStaking contracts.

use alloy::sol;

sol! {
    /// SubgraphService contract for Horizon (V2) allocation management.
    ///
    /// This contract handles:
    /// - Starting service (creating allocations)
    /// - Collecting indexing rewards
    /// - Stopping service (closing allocations)
    #[sol(rpc)]
    contract SubgraphService {
        /// Start providing service for a subgraph deployment.
        /// This creates a new allocation.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        /// * `data` - Encoded service data (deployment ID, tokens, allocation ID, proof)
        function startService(address indexer, bytes calldata data) external;

        /// Collect payment for provided service.
        /// For indexing rewards, this may close the allocation if over-allocated.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        /// * `paymentType` - Type of payment (0 = IndexingRewards)
        /// * `data` - Encoded collection data
        function collect(address indexer, uint8 paymentType, bytes calldata data) external returns (uint256);

        /// Stop providing service for an allocation.
        /// Must be called after collect() if not over-allocated.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        /// * `data` - Encoded stop data (allocation ID)
        function stopService(address indexer, bytes calldata data) external;

        /// Execute multiple calls in a single transaction.
        ///
        /// # Arguments
        /// * `data` - Array of encoded function calls
        function multicall(bytes[] calldata data) external returns (bytes[] memory results);

        /// Get allocation details.
        ///
        /// # Arguments
        /// * `allocationId` - The allocation ID
        function getAllocation(address allocationId) external view returns (Allocation memory);

        /// Check if an indexer is over-allocated.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        function isOverAllocated(address indexer) external view returns (bool);

        /// Allocation data structure
        struct Allocation {
            address indexer;
            bytes32 subgraphDeploymentId;
            uint256 tokens;
            uint256 createdAt;
            uint256 closedAt;
            uint256 lastPOISubmittedAt;
            uint256 accRewardsPerAllocatedToken;
            uint256 accRewardsPending;
        }

        /// Emitted when an allocation is created
        event AllocationCreated(
            address indexed indexer,
            address indexed allocationId,
            bytes32 indexed subgraphDeploymentId,
            uint256 tokens,
            uint256 currentEpoch
        );

        /// Emitted when an allocation is closed
        event AllocationClosed(
            address indexed indexer,
            address indexed allocationId,
            bytes32 indexed subgraphDeploymentId,
            uint256 tokens,
            bool forceClosed
        );

        /// Emitted when indexing rewards are collected
        event IndexingRewardsCollected(
            address indexed indexer,
            address indexed allocationId,
            bytes32 indexed subgraphDeploymentId,
            uint256 tokensRewards,
            uint256 tokensIndexerRewards,
            uint256 tokensDelegationRewards,
            bytes32 poi
        );
    }

    /// HorizonStaking contract for stake management and legacy operations.
    #[sol(rpc)]
    contract HorizonStaking {
        /// Add tokens to an existing provision.
        /// Used during Horizon transition to move stake to SubgraphService.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        /// * `serviceProvider` - The service provider contract address
        /// * `tokens` - Amount of tokens to add
        function addToProvision(address indexer, address serviceProvider, uint256 tokens) external;

        /// Get provision details for an indexer and service provider.
        ///
        /// # Arguments
        /// * `indexer` - The indexer's address
        /// * `serviceProvider` - The service provider contract address
        function getProvision(address indexer, address serviceProvider) external view returns (Provision memory);

        /// Close an allocation (legacy method, pre-Horizon).
        ///
        /// # Arguments
        /// * `allocationId` - The allocation ID to close
        /// * `poi` - Proof of Indexing
        function closeAllocation(address allocationId, bytes32 poi) external;

        /// Provision data structure
        struct Provision {
            uint256 tokens;
            uint256 tokensThawing;
            uint256 sharesThawing;
            uint32 maxVerifierCut;
            uint64 thawingPeriod;
            uint64 createdAt;
            uint32 maxVerifierCutPending;
            uint64 thawingPeriodPending;
        }
    }
}

/// Payment types for the collect function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PaymentType {
    /// Indexing rewards payment
    IndexingRewards = 0,
    /// Query fees payment
    QueryFees = 1,
}

impl From<PaymentType> for u8 {
    fn from(value: PaymentType) -> Self {
        value as u8
    }
}

/// Re-export contract types for external use.
pub use HorizonStaking as HorizonStakingContract;
pub use SubgraphService as SubgraphServiceContract;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payment_type_conversion() {
        assert_eq!(u8::from(PaymentType::IndexingRewards), 0);
        assert_eq!(u8::from(PaymentType::QueryFees), 1);
    }
}
