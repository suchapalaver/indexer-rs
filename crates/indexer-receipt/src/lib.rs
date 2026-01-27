// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! TAP receipt types for V2/Horizon protocol.
//!
//! V1/Legacy TAP support has been removed.

use tap_core::{
    receipt::{
        rav::{Aggregate, AggregationError},
        WithUniqueId, WithValueAndTimestamp,
    },
    signed_message::SignatureBytes,
};
use thegraph_core::alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Address, FixedBytes},
    signers::Signature,
};

/// A TAP receipt (V2/Horizon only).
///
/// V1/Legacy TAP support has been removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TapReceipt {
    /// V2 (Horizon) receipt
    V2(tap_graph::v2::SignedReceipt),
}

impl From<tap_graph::v2::SignedReceipt> for TapReceipt {
    fn from(receipt: tap_graph::v2::SignedReceipt) -> Self {
        Self::V2(receipt)
    }
}

impl Aggregate<TapReceipt> for tap_graph::v2::ReceiptAggregateVoucher {
    fn aggregate_receipts(
        receipts: &[tap_core::receipt::ReceiptWithState<
            tap_core::receipt::state::Checked,
            TapReceipt,
        >],
        previous_rav: Option<tap_core::signed_message::Eip712SignedMessage<Self>>,
    ) -> Result<Self, tap_core::receipt::rav::AggregationError> {
        if receipts.is_empty() {
            return Err(AggregationError::NoValidReceiptsForRavRequest);
        }
        let receipts: Vec<_> = receipts
            .iter()
            .map(|receipt| {
                let TapReceipt::V2(r) = receipt.signed_receipt();
                r.clone()
            })
            .collect();
        let collection_id = receipts[0].message.collection_id;
        let payer = receipts[0].message.payer;
        let data_service = receipts[0].message.data_service;
        let service_provider = receipts[0].message.service_provider;

        tap_graph::v2::ReceiptAggregateVoucher::aggregate_receipts(
            collection_id,
            payer,
            data_service,
            service_provider,
            receipts.as_slice(),
            previous_rav,
        )
    }
}

impl TapReceipt {
    /// Get a reference to the inner V2 receipt.
    pub fn inner(&self) -> &tap_graph::v2::SignedReceipt {
        let Self::V2(receipt) = self;
        receipt
    }

    /// Consume and return the inner V2 receipt.
    pub fn into_inner(self) -> tap_graph::v2::SignedReceipt {
        let Self::V2(receipt) = self;
        receipt
    }

    /// Consume and return the inner V2 receipt (alias for `into_inner`).
    ///
    /// This method provides a consistent API for extracting the V2 receipt.
    pub fn as_v2(self) -> tap_graph::v2::SignedReceipt {
        self.into_inner()
    }

    /// Get a reference to the inner V2 receipt (alias for `inner`).
    pub fn get_v2_receipt(&self) -> &tap_graph::v2::SignedReceipt {
        self.inner()
    }

    /// Get the collection ID from the receipt.
    pub fn collection_id(&self) -> FixedBytes<32> {
        let Self::V2(receipt) = self;
        receipt.message.collection_id
    }

    /// Get the payer address from the receipt.
    pub fn payer(&self) -> Address {
        let Self::V2(receipt) = self;
        receipt.message.payer
    }

    /// Get the data service address from the receipt.
    pub fn data_service(&self) -> Address {
        let Self::V2(receipt) = self;
        receipt.message.data_service
    }

    /// Get the service provider address from the receipt.
    pub fn service_provider(&self) -> Address {
        let Self::V2(receipt) = self;
        receipt.message.service_provider
    }

    /// Get the signature from the receipt.
    pub fn signature(&self) -> Signature {
        let Self::V2(receipt) = self;
        receipt.signature
    }

    /// Get the nonce from the receipt.
    pub fn nonce(&self) -> u64 {
        let Self::V2(receipt) = self;
        receipt.message.nonce
    }

    /// Recover the signer address from the receipt signature.
    pub fn recover_signer(
        &self,
        domain_separator: &Eip712Domain,
    ) -> Result<Address, tap_core::signed_message::Eip712Error> {
        let Self::V2(receipt) = self;
        receipt.recover_signer(domain_separator)
    }
}

impl WithValueAndTimestamp for TapReceipt {
    fn value(&self) -> u128 {
        let Self::V2(receipt) = self;
        receipt.value()
    }

    fn timestamp_ns(&self) -> u64 {
        let Self::V2(receipt) = self;
        receipt.timestamp_ns()
    }
}

impl WithUniqueId for TapReceipt {
    type Output = SignatureBytes;

    fn unique_id(&self) -> Self::Output {
        let Self::V2(receipt) = self;
        receipt.unique_id()
    }
}
