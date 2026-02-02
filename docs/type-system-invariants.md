# Type-System Invariant Catalog (indexer-rs)

Purpose: enumerate invariants and type-debt hotspots where Rust's type system can replace runtime checks, with zero-cost abstractions. This catalog drives the phased refactor (errors first, then units/IDs, action inputs, lifecycle, proofs, POI).

Scope: indexer-rs only. Logs should remain stable. Breaking internal APIs are OK as long as config/CLI/operational behavior stays compatible.

## 1) Domain Map (high-value typed surfaces)

### 1.1 Action inputs and queueing
- Primary structs and flow:
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/indexer-agent/src/reconciliation/actions.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
- Current state:
  - `ActionInput` and `Action` use `String` for deployment_id, allocation_id, amount, protocol_network, poi.
  - Action type and status are enums but not state-machine enforced.
- Type-system opportunity:
  - Introduce typed input constructors per action kind (Allocate/Unallocate/Reallocate).
  - Replace `String` IDs with newtypes.

### 1.2 Allocation lifecycle
- Primary structs:
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
  - `crates/allocation/src/lib.rs`
- Current state:
  - Lifecycle is implicit in status strings and runner control flow.
  - On-chain allocation verification is runtime.

### 1.3 Proofs and signing
- Primary code:
  - `crates/indexer-agent/src/executor/contracts.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
- Current state:
  - EIP-712 domain and struct defined, proof constructed at runtime.
- Type-system opportunity:
  - Separate proof types by scheme (EIP-712 vs legacy) with distinct APIs.

### 1.4 Amounts and units
- Primary code:
  - `crates/indexer-agent/src/executor/transactions.rs`
  - `crates/indexer-agent/src/validation.rs`
  - `crates/config/src/grt.rs`
- Current state:
  - Strings parsed into U256, GRT vs wei is runtime policy.
  - `NonZeroGRT` is typed in config, but not propagated into domain.

### 1.5 Chain IDs / protocol networks
- Primary code:
  - `crates/indexer-agent/src/validation.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
- Current state:
  - `protocol_network` is `String`, `chain_id` is `u64`, validation is runtime.

### 1.6 Addresses and IDs
- Primary code:
  - `crates/indexer-agent/src/executor/runner.rs`
  - `crates/indexer-agent/src/executor/contracts.rs`
  - `crates/allocation/src/lib.rs`
- Current state:
  - `Address` is used in executor and allocation types, but most models use `String`.
  - Allocation ID uniqueness/derivation is runtime.

### 1.7 POI semantics
- Primary code:
  - `crates/indexer-agent/src/executor/runner.rs`
  - `crates/indexer-agent/src/executor/transactions.rs`
- Current state:
  - `poi: Option<String>`, `poi_block_number: Option<i32>`, `public_poi: Option<String>`.
  - Force-close uses zero POI at runtime.

### 1.8 Errors
- Primary code:
  - `crates/indexer-agent/src/executor/errors.rs`
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/service/src/error.rs`
  - `crates/tap-agent/src/tap/context/error.rs`
  - `crates/monitor/src/client/subgraph_client.rs`
- Current state:
  - Multiple error enums; some error data is stringly-typed.
  - Mixed `anyhow`/custom errors, limiting compile-time guarantees.

## 2) Invariant Catalog (by area)

### 2.1 Action queueing invariants
- One pending action per deployment (DB constraint):
  - `Action::queue` handles unique index `idx_one_pending_action_per_deployment`.
  - `crates/indexer-agent/src/models/action.rs`
- Action cooldown invariant (HP-1 / Invariant 8.2/22.2):
  - Enforced in reconciliation; prevents rapid re-queueing.
  - `crates/indexer-agent/src/reconciliation/actions.rs`
- Legacy actions must be rejected (Horizon-only):
  - `Action::queue` and validation reject `is_legacy = true`.
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/indexer-agent/src/validation.rs`
- Invalid inputs must not stall reconciliation:
  - Reconciliation logs and skips invalid actions, increments a metric.
  - `crates/indexer-agent/src/reconciliation/actions.rs`
  - `crates/indexer-agent/src/metrics.rs`

### 2.2 Allocation lifecycle invariants
- Execution order: Unallocate -> Reallocate -> Allocate.
  - `crates/indexer-agent/src/executor/runner.rs`
- Reallocate is non-atomic (two transactions):
  - If unallocate succeeds and allocate fails, stake returns to pool.
  - Operator must re-allocate manually or retry.
  - Metric `agent_reallocate_partial_failures_total` tracks partial failures.
  - `crates/indexer-agent/src/executor/runner.rs`
  - `crates/indexer-agent/src/metrics.rs`
- Allocation must exist and be valid before unallocating:
  - `verify_allocation_exists` checks on chain.
  - `crates/indexer-agent/src/executor/runner.rs`
- Allocation ID derivation:
  - Derived from operator mnemonic + epoch + deployment + index.
  - Indices tried in bounded range (0..MAX_ALLOCATION_INDEX).
  - `crates/indexer-agent/src/executor/runner.rs`
- Allocation ID uniqueness on chain:
  - Derived ID must not already exist (indexer != 0x0).
  - `crates/indexer-agent/src/executor/runner.rs`

### 2.3 Proof invariants (Horizon)
- Allocation proof must be an EIP-712 signature over:
  - `AllocationIdProof(indexer, allocationId)`
  - Domain: name "SubgraphService", version "1.0", chainId, verifying contract.
  - `crates/indexer-agent/src/executor/contracts.rs`
  - `crates/indexer-agent/src/executor/runner.rs`

### 2.4 Amounts/units invariants
- Amount policy (current):
  - Integer strings => wei; decimal/scientific => GRT converted to wei.
  - Must be non-zero and representable as U256.
  - `crates/indexer-agent/src/executor/transactions.rs`
  - `crates/indexer-agent/src/validation.rs`
- Config `NonZeroGRT` ensures >0 and exact wei in u128:
  - `crates/config/src/grt.rs`

### 2.5 Protocol network invariants
- Protocol network is CAIP-2 format:
  - `namespace:reference`, with specific charset/length.
  - `crates/indexer-agent/src/validation.rs`
- Chain ID used for provider selection and EIP-712 domain:
  - `crates/indexer-agent/src/executor/runner.rs`

### 2.6 Deployment ID invariants
- Deployment ID format:
  - CIDv0 (Qm...) or bytes32 hex.
  - `crates/indexer-agent/src/validation.rs`
  - `crates/indexer-agent/src/executor/runner.rs` (`parse_deployment_id`)

### 2.7 POI invariants
- Proof of Indexing (POI) required for unallocation; missing POI => forced close via zero bytes32.
  - `crates/indexer-agent/src/executor/runner.rs`
- Explicit POI submissions require a POI block number > 0.
- Public POI is only allowed when POI is provided.
- POI metadata includes block number and optional public POI:
  - `crates/indexer-agent/src/executor/transactions.rs`
  - `crates/indexer-agent/src/validation.rs`

### 2.8 Gas price threshold invariant
- Invariant 25.2: gas price threshold check; waits until under limit or timeout.
  - `crates/indexer-agent/src/executor/runner.rs`

## 3) Type-Debt Map (top priority)

### 3.1 Stringly-typed identifiers
- `Action`/`ActionInput` fields (deployment_id, allocation_id, amount, poi, protocol_network) are strings.
  - `crates/indexer-agent/src/models/action.rs`
- Risk: runtime parsing and implicit conversions in executor.

### 3.2 Allocation lifecycle not encoded
- Action status is enum but transitions are not enforced at type level.
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
- Risk: invalid transitions or inconsistent states.

### 3.3 Proof APIs not type-separated
- EIP-712 vs other schemes share `Bytes`/`String` values.
  - `crates/indexer-agent/src/executor/contracts.rs`
  - `crates/indexer-agent/src/executor/runner.rs`

### 3.4 Amounts and units not encoded as types
- `String` input across reconciliation and executor.
  - `crates/indexer-agent/src/reconciliation/actions.rs`
  - `crates/indexer-agent/src/executor/runner.rs`
- Risk: unit confusion and precision errors.

### 3.5 Chain/network types
- `protocol_network: String`, `chain_id: u64`.
  - `crates/indexer-agent/src/executor/runner.rs`
- Risk: invalid values propagated until runtime.

### 3.6 Error fragmentation
- Multiple error enums with string payloads; mixed `anyhow`.
  - `crates/indexer-agent/src/executor/errors.rs`
  - `crates/indexer-agent/src/models/action.rs`
  - `crates/service/src/error.rs`
  - `crates/tap-agent/src/tap/context/error.rs`
  - `crates/monitor/src/client/subgraph_client.rs`

## 4) Proposed Type Palette (zero-cost)

- `#[repr(transparent)]` newtypes:
  - `Wei(U256)`, `GRT(BigDecimal or fixed-point)`, `ChainId(u64)`, `ProtocolNetwork(String)`, `DeploymentId(thegraph_core::DeploymentId)`, `AllocationId(Address)`, `IndexerAddress(Address)`, `TxHash(FixedBytes<32>)`, `BlockNumber(u64)`, `POI(FixedBytes<32>)`.
- Strongly-typed action input variants:
  - `AllocateInput { deployment_id, amount, ... }`
  - `UnallocateInput { allocation_id, poi, poi_block_number, ... }`
  - `ReallocateInput { allocation_id, amount, ... }`

## 5) Error Taxonomy (errors-first plan)

- Error classes:
  - `InputError` (invalid format, missing fields, validation).
  - `InvariantViolation` (unexpected internal state).
  - `ExternalError` (db/chain/provider/network).
  - `InternalBug` (should be unreachable).
- Goals:
  - Preserve log strings via `Display` implementations.
  - Reduce `String` error payloads.
  - Centralize conversions from external errors.

## 6) Proptest Targets (initial set)
- Parsing/round-trip:
  - Deployment ID (CIDv0, bytes32 hex).
  - Amount strings (wei vs GRT) -> exact U256.
  - Protocol network CAIP-2 parsing.
- Invariants:
  - Action state transitions (legal/illegal).
  - Allocation lifecycle constraints (no unallocate without allocation).

## 7) Migration Constraints
- Config/CLI behavior must remain compatible.
- Logs should remain stable; error strings preserved.
- Prefer zero-alloc, zero-cost wrappers.
