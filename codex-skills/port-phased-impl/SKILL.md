---
name: port-phased-impl
description: Phased implementation plan, migration guidance, and security audit for the TS indexer invariants already surfaced in indexer-rs. Use for a high-agency phased rollout that preserves invariants.
---

# Port Phased Implementation (High-Agency)

## Purpose

Drive a phased implementation plan and security audit for the specific invariants already surfaced in indexer-rs. Keep scope strictly to these deltas:

- Reallocate non-atomicity (partial failure visibility and recovery)
- Epoch transition race in reconciliation loop
- Management API default bind hardening (localhost)
- Allocation proof signing correctness (EIP-712)
- Amount parsing ambiguity (GRT vs wei, float precision, overflow)
- Action input validation and action transaction immutability
- POI resolution and force-close semantics

## Operating Rules

- Do not expand scope beyond the surfaced issues.
- Be high-agency and decisive; propose concrete phases with acceptance criteria.
- Tie every phase to explicit invariants and failure modes.
- Prefer minimal changes that preserve on-chain safety and TS parity.
- Always call out risks to indexer funds and revert paths.

## Workflow

### Phase 0: Baseline & Safety Gates (no code changes yet)

- Summarize the surfaced issues and map them to invariants from `../indexer/docs/INVARIANTS.md`.
- Define objective success metrics for each issue (e.g., on-chain tx succeeds; no allocation ID collisions; POI path matches TS).
- Establish a rollback plan per phase.
- Identify where tests will live (unit, integration, e2e with edgeandnode/local-network).

### Phase 1: Consensus-Critical Fixes (must not risk funds)

**Goal:** Prevent chain tx failures and catastrophic mis-allocations.

- Fix allocation proof signing to match Horizon EIP-712.
- Fix amount parsing to be unambiguous and lossless (no f64). Define accepted formats explicitly, include overflow/large exponent tests.
- Make reallocate partial failures explicit (record both tx hashes or partial status; no silent loss of audit trail).

Acceptance criteria:

- Allocation tx executes in local-network with valid proof.
- Amount parsing for wei and GRT is correct for large values and rejects ambiguous input.
- Reallocate partial failure is recoverable and auditable (no missing unallocate hash).

### Phase 2: Invariant Enforcement & Action Validations

**Goal:** Prevent invalid actions from entering the queue and enforce invariants early.

- Enforce action input validation (required params per action type, protocol network format).
- Enforce transaction immutability for Actions once set.
- Harden Management API default bind to localhost.

Acceptance criteria:

- Queueing invalid actions fails at API boundary.
- Transaction hash cannot be overwritten once set.
- Management API does not bind to 0.0.0.0 by default.

### Phase 3: Reconciliation Safety & POI Semantics

**Goal:** Preserve close/unallocate invariants and rewards safety under live state changes.

- Fix epoch transition race in reconciliation loop (bounded retry or explicit invariant for single retry).
- Implement POI resolution path or enforce POI presence for unallocate/reallocate, including public POI and block number.
- Honor `force` semantics rather than forcing zero-POI by default.

Acceptance criteria:

- Reconciliation handles epoch transitions without inconsistent snapshots.
- Unallocate uses resolved POI when provided; force-close only when explicitly requested.
- POI metadata encoded matches TS expectations.

### Phase 4: End-to-End Validation

**Goal:** Prove functional parity and safety.

- Add or extend tests mirroring TS invariants (unit + e2e on local-network).
- Run reconciliation loop tests that cover allocation create/close/reallocate across a restart.
- Validate management API action queues with invalid inputs.

Acceptance criteria:

- All new tests pass; e2e shows parity for critical flows.
- No unexpected funds movement or failed transactions in e2e.

## Execution Pattern (per phase)

1) Re-state the invariant(s) being enforced.
2) Identify minimal code deltas and touched files.
3) Propose targeted tests and expected outputs.
4) Provide a go/no-go checklist.

## Output Format

- Start with a crisp phase plan.
- Then list concrete changes by file path and invariant.
- End with a risk log and test plan for the phase.

## Guardrails

- Never suggest or perform destructive git operations.
- If you discover unrelated diffs or unexpected changes, stop and ask the user.
- Keep recommendations strictly aligned to the surfaced issues only.
