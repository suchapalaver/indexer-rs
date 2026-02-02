# Alloy Standards & Phased Implementation

## Goal
Build an S-tier, Horizon-only indexer stack with Alloy-first, compile-time-safe EVM interactions. Optimize for correctness, auditability, and operational stability while preserving config/CLI compatibility.

## Non-Goals
- Reintroducing TAP v1/legacy behavior.
- Relaxing validation or relying on runtime-only checks where types can enforce invariants.

## Alloy Standards (Repo-Specific)
- **Providers**: Use `ProviderBuilder::new()` (alloy 1.0.42 includes recommended fillers) + wallet fillers; keep per-(chain, signer) nonce isolation. Avoid `ProviderBuilder::default()` in production code.
- **Contracts**: Use `sol!` and strongly typed call structs; avoid raw ABI encoding when typed calls are available.
- **Addresses**: Use `address!` for literals; parse external strings at the boundary only.
- **Types**: Prefer `BlockNumber`, `ChainId`, `TxHash`, `ProofOfIndexing`, `AllocationId`, `IndexerId` newtypes over primitives. Convert once at boundaries.
- **EIP-712**: Centralize domain creation; ensure typed hashes and signing are tested against known vectors.
- **Errors**: Classify as Input / Invariant / External; redact external input in error messages.
- **Testing**: Test our logic, not Alloy. Use deterministic wallets/fixtures and avoid real network calls in unit tests.

## Phased Implementation Plan

### Phase 1 — Executor + On-Chain Safety (Highest Risk)
**Focus**: `crates/indexer-agent/src/executor/*`
- Make contract interactions exclusively typed (`sol!`) and document any raw ABI usage.
- Strengthen EIP-712 proof tests (domain, type hash, signing hash, recovery).
- Ensure provider cache and nonce safety remain deterministic across reorgs.
- Add invariants for transaction audit trails (e.g., store unallocate+allocate hashes).
**Acceptance**: unit tests for proof generation, tx encoding, and error classification; no runtime parsing of literals in executor logic.

### Phase 2 — TAP Agent + Receipts/RAVs
**Focus**: `crates/tap-agent/*`, `crates/service/src/tap/*`
- Ensure receipt/rav parsing uses typed addresses and typed POI.
- Remove any implicit stringly-typed paths for allocation IDs or sender IDs.
- Confirm EIP-712 domain separation is consistent across receipt creation/validation.
**Acceptance**: receipt and RAV tests cover invalid inputs and explicit POI flows; no unit tests making real network calls.

### Phase 3 — Management API + Validation
**Focus**: `crates/indexer-management-api/*`, `crates/indexer-agent/src/validation.rs`
- Enforce strict input validation at the API boundary (addresses, deployment IDs, amounts).
- Map API inputs into typed wrappers immediately after validation.
- Ensure error redaction for user inputs and deterministic error classes.
**Acceptance**: GraphQL mutations rejected for malformed inputs; explicit POI behavior verified end-to-end through executor.

### Phase 4 — Local-Network / Integration Validation
**Focus**: `integration-tests/`, `docs/phase-4-local-network.md`
- Run Horizon-only local-network flows with unified binary.
- Validate allocation lifecycle, POI submission, and payout signals in the full stack.
- Record and freeze runbook steps for repeatable ops.
**Acceptance**: Phase 4 runbook passes on Linux/x86; results captured and checked in as docs/logs as appropriate.

## PR Checklist (Alloy-Related)
- All literal EVM addresses use `address!`.
- Only boundary layers parse strings into `Address`/`BlockNumber`/`ChainId`.
- EIP-712 domains and hashes tested against known values.
- Error messages redact user-supplied inputs.
- Unit tests remain deterministic with no external network calls.
