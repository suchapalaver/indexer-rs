// Copyright 2023-, Edge & Node, GraphOps, and Semiotic Labs.
// SPDX-License-Identifier: Apache-2.0

//! Prometheus metrics for the indexer agent.
//!
//! This module provides observability into:
//! - Reconciliation loop health and performance
//! - Action executor transaction outcomes
//! - Action queue behavior including cooldown enforcement

use std::sync::LazyLock;

use prometheus::{
    register_counter_vec, register_histogram_vec, register_int_counter_vec, CounterVec,
    HistogramVec, IntCounterVec,
};

// =============================================================================
// Reconciliation Loop Metrics
// =============================================================================

/// Total number of reconciliation cycles completed.
///
/// Labels:
/// - `status`: "success" or "error"
pub static RECONCILIATION_CYCLES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "agent_reconciliation_cycles_total",
        "Total number of reconciliation cycles completed",
        &["status"]
    )
    .expect("failed to register agent_reconciliation_cycles_total metric")
});

/// Duration of reconciliation cycles in seconds.
pub static RECONCILIATION_DURATION_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "agent_reconciliation_duration_seconds",
        "Duration of reconciliation cycles in seconds",
        &["status"],
        vec![0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]
    )
    .expect("failed to register agent_reconciliation_duration_seconds metric")
});

/// Total number of actions queued by the reconciliation loop.
///
/// Labels:
/// - `action_type`: "allocate", "unallocate", or "reallocate"
pub static RECONCILIATION_ACTIONS_QUEUED_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "agent_reconciliation_actions_queued_total",
        "Total number of actions queued by the reconciliation loop",
        &["action_type"]
    )
    .expect("failed to register agent_reconciliation_actions_queued_total metric")
});

/// Total number of actions skipped due to cooldown.
///
/// This metric tracks when the action cooldown (HP-1/Invariant 8.2) prevents
/// re-queuing an action for a deployment that recently had an action executed.
///
/// Labels:
/// - `action_type`: "allocate", "unallocate", or "reallocate"
pub static ACTIONS_COOLDOWN_SKIPPED_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "agent_actions_cooldown_skipped_total",
        "Total number of actions skipped due to cooldown period",
        &["action_type"]
    )
    .expect("failed to register agent_actions_cooldown_skipped_total metric")
});

// =============================================================================
// Executor Metrics
// =============================================================================

/// Total number of transactions submitted by the executor.
///
/// Labels:
/// - `action_type`: "allocate", "unallocate", or "reallocate"
/// - `status`: "success", "failed", or "reverted"
pub static EXECUTOR_TRANSACTIONS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "agent_executor_transactions_total",
        "Total number of transactions submitted by the executor",
        &["action_type", "status"]
    )
    .expect("failed to register agent_executor_transactions_total metric")
});

/// Gas used by executor transactions (in wei).
///
/// Labels:
/// - `action_type`: "allocate", "unallocate", or "reallocate"
pub static EXECUTOR_GAS_USED: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "agent_executor_gas_used_wei",
        "Gas used by executor transactions in wei",
        &["action_type"],
        // Buckets from 100k to 10M gas (in wei, so multiply by typical gas price)
        vec![
            100_000.0,
            200_000.0,
            500_000.0,
            1_000_000.0,
            2_000_000.0,
            5_000_000.0,
            10_000_000.0
        ]
    )
    .expect("failed to register agent_executor_gas_used_wei metric")
});

/// Time spent waiting for acceptable gas price (HP-3/Invariant 25.2).
///
/// This histogram tracks how long the executor waited before submitting
/// a transaction when the gas price was above the configured threshold.
/// A value of 0 means no waiting was required.
pub static EXECUTOR_GAS_PRICE_WAIT_SECONDS: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "agent_executor_gas_price_wait_seconds",
        "Time spent waiting for acceptable gas price",
        &["outcome"], // "proceeded" or "timeout"
        vec![0.0, 15.0, 30.0, 60.0, 120.0, 180.0, 300.0]
    )
    .expect("failed to register agent_executor_gas_price_wait_seconds metric")
});

/// Total number of gas price wait events.
///
/// Labels:
/// - `outcome`: "immediate" (no wait needed), "waited" (waited and proceeded), or "timeout"
pub static EXECUTOR_GAS_PRICE_WAITS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "agent_executor_gas_price_waits_total",
        "Total number of gas price wait events",
        &["outcome"]
    )
    .expect("failed to register agent_executor_gas_price_waits_total metric")
});

/// Current gas price observed by the executor (in gwei).
///
/// This gauge is updated each time the executor checks gas price.
pub static EXECUTOR_CURRENT_GAS_PRICE_GWEI: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "agent_executor_gas_price_gwei",
        "Current gas price observed by executor in gwei (use rate() for changes)",
        &[]
    )
    .expect("failed to register agent_executor_gas_price_gwei metric")
});

// =============================================================================
// Helper Functions
// =============================================================================

/// Record a successful reconciliation cycle.
pub fn record_reconciliation_success(duration_secs: f64) {
    RECONCILIATION_CYCLES_TOTAL
        .with_label_values(&["success"])
        .inc();
    RECONCILIATION_DURATION_SECONDS
        .with_label_values(&["success"])
        .observe(duration_secs);
}

/// Record a failed reconciliation cycle.
pub fn record_reconciliation_error(duration_secs: f64) {
    RECONCILIATION_CYCLES_TOTAL
        .with_label_values(&["error"])
        .inc();
    RECONCILIATION_DURATION_SECONDS
        .with_label_values(&["error"])
        .observe(duration_secs);
}

/// Record an action being queued.
pub fn record_action_queued(action_type: &str) {
    RECONCILIATION_ACTIONS_QUEUED_TOTAL
        .with_label_values(&[action_type])
        .inc();
}

/// Record an action being skipped due to cooldown.
pub fn record_action_cooldown_skip(action_type: &str) {
    ACTIONS_COOLDOWN_SKIPPED_TOTAL
        .with_label_values(&[action_type])
        .inc();
}

/// Record a successful transaction.
pub fn record_transaction_success(action_type: &str, gas_used: u64) {
    EXECUTOR_TRANSACTIONS_TOTAL
        .with_label_values(&[action_type, "success"])
        .inc();
    EXECUTOR_GAS_USED
        .with_label_values(&[action_type])
        .observe(gas_used as f64);
}

/// Record a failed transaction (submission or receipt error).
pub fn record_transaction_failed(action_type: &str) {
    EXECUTOR_TRANSACTIONS_TOTAL
        .with_label_values(&[action_type, "failed"])
        .inc();
}

/// Record a reverted transaction.
pub fn record_transaction_reverted(action_type: &str, gas_used: u64) {
    EXECUTOR_TRANSACTIONS_TOTAL
        .with_label_values(&[action_type, "reverted"])
        .inc();
    EXECUTOR_GAS_USED
        .with_label_values(&[action_type])
        .observe(gas_used as f64);
}

/// Record gas price wait - no wait needed.
pub fn record_gas_price_immediate() {
    EXECUTOR_GAS_PRICE_WAITS_TOTAL
        .with_label_values(&["immediate"])
        .inc();
}

/// Record gas price wait - waited and proceeded.
pub fn record_gas_price_waited(wait_secs: f64) {
    EXECUTOR_GAS_PRICE_WAITS_TOTAL
        .with_label_values(&["waited"])
        .inc();
    EXECUTOR_GAS_PRICE_WAIT_SECONDS
        .with_label_values(&["proceeded"])
        .observe(wait_secs);
}

/// Record gas price wait - timeout.
pub fn record_gas_price_timeout(wait_secs: f64) {
    EXECUTOR_GAS_PRICE_WAITS_TOTAL
        .with_label_values(&["timeout"])
        .inc();
    EXECUTOR_GAS_PRICE_WAIT_SECONDS
        .with_label_values(&["timeout"])
        .observe(wait_secs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_register() {
        // Verify all metrics can be registered without panicking
        let _ = &*RECONCILIATION_CYCLES_TOTAL;
        let _ = &*RECONCILIATION_DURATION_SECONDS;
        let _ = &*RECONCILIATION_ACTIONS_QUEUED_TOTAL;
        let _ = &*ACTIONS_COOLDOWN_SKIPPED_TOTAL;
        let _ = &*EXECUTOR_TRANSACTIONS_TOTAL;
        let _ = &*EXECUTOR_GAS_USED;
        let _ = &*EXECUTOR_GAS_PRICE_WAIT_SECONDS;
        let _ = &*EXECUTOR_GAS_PRICE_WAITS_TOTAL;
    }

    #[test]
    fn test_record_helpers() {
        // Verify helper functions don't panic
        record_reconciliation_success(1.5);
        record_reconciliation_error(0.5);
        record_action_queued("allocate");
        record_action_cooldown_skip("unallocate");
        record_transaction_success("allocate", 200_000);
        record_transaction_failed("unallocate");
        record_transaction_reverted("reallocate", 150_000);
        record_gas_price_immediate();
        record_gas_price_waited(30.0);
        record_gas_price_timeout(300.0);
    }
}
