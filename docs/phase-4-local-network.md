# Phase 4 Plan: Local-Network Integration (Post-Horizon)

Purpose: prove the single-binary, Horizon-only indexer-rs stack is production-safe end-to-end using the same local-network flow operators will run. This phase validates explicit POI, allocation lifecycle, reconciliation, and outage recovery with stable logs and deterministic tests.

Scope: indexer-rs only; Horizon-only (no legacy/v1). Keep CLI/config behavior compatible. Logs should remain stable. Favor zero-cost types and minimal runtime checks.

## 1) Environment Baseline (local-network)
- Source of truth: `setup-test-network.sh` and `integration-tests/INTEGRATION_TESTING_INSTRUCTIONS.md`.
- Ensure local-network matches post-Horizon assumptions:
  - No v1 TAP paths, no legacy receipts.
  - Escrow funding, graph-node readiness, and subgraph deployment checks succeed.
- Confirm required services are healthy before tests (graph-node, indexer-service, tap components).
 - Note: `integration-tests/INTEGRATION_TESTING_INSTRUCTIONS.md` still documents V1 steps; ignore all V1/legacy sections.

## 2) Integration Test Matrix (Horizon-only)
- Run the v2/Horizon path only:
  - allocate → receipts → RAV → close allocation
  - denylist and payer checks
  - escrow funding and withdrawal
- Validate management API queueing with explicit POI:
  - enqueue POI + block number via management API queue path
  - assert invalid POI inputs are rejected

## 3) Reconciliation + Recovery
- Outage simulation:
  - stop indexer during receipt intake
  - restart and verify reconciliation completes
- Ensure reconciliation skips invalid inputs (no stalls) and emits the metric.
- Verify reallocate non-atomic path logs and metric are stable.
  - Metric: `agent_reallocate_partial_failures_total`
  - Operator response: inspect failed allocation, confirm unallocate tx hash, re-queue allocation once conditions are safe.

## 4) Observability & Stability
- Confirm:
  - stable log strings for allocation lifecycle, POI handling, and reconciliation
  - no noisy retry loops or unbounded backoff in tests
  - metrics for reallocate partial failures and invalid inputs are emitted

## 5) Documentation Closure
- Add a short operator checklist under docs/:
  - local-network setup, Horizon-only constraints
  - exact integration test sequence
  - expected artifacts (allocations, receipts, RAVs, POI)

## 6) Local-Network Runbook (step-by-step)
1) Start the local-network:
   - Use `setup-test-network.sh` to stand up the Horizon local-network stack.
   - Confirm readiness checks complete (escrow funding, graph-node health, subgraph deployment).

2) Switch to unified (single-binary) mode:
   - Ensure the standalone `tap-agent` container is stopped.
     - Example: `docker stop tap-agent && docker rm tap-agent`
   - Enable integrated TAP agent in config:
     - `agent.enabled = true`
     - `agent.tap_agent_enabled = true`
     - `agent.management_api.host = "127.0.0.1"`
   - Start only `indexer-service` (unified binary) and verify `/health`.

3) Run the Horizon integration path:
   - Follow `integration-tests/INTEGRATION_TESTING_INSTRUCTIONS.md`.
   - Execute the v2/Horizon path only (skip any legacy/v1 steps).

4) Validate explicit POI queue path:
   - Submit an explicit POI + block number through the management API queue path.
   - Verify the action executes and the POI is recorded.
   - Confirm invalid POI inputs are rejected (missing or zero block number).

5) Exercise allocation lifecycle:
   - Allocate, collect receipts, aggregate RAV, close allocation.
   - Confirm denylist and payer checks behave as expected.

6) Outage recovery:
   - Stop indexer-rs during receipt intake.
   - Restart and verify reconciliation completes and backlog drains.

7) Verify observability:
   - Logs remain stable (no new noisy errors).
   - Metrics present for invalid inputs and reallocate partial failures (`agent_reallocate_partial_failures_total`).

8) Capture evidence:
   - Record the test sequence, timestamps, and any deviations.
   - Update docs with any new operational constraints discovered.

## Acceptance Criteria
- Local-network run passes with Horizon-only flows.
- Explicit POI path validated end-to-end (queue → execute → record).
- Reconciliation recovers from outage and does not stall on invalid input.
- Logs stable; metrics emitted; tests deterministic.

## Phase 4 Execution Checklist
Pre-flight:
- `just down` (clean slate)
- `just setup` (or `./setup-test-network.sh`) and wait for all health checks
- Verify Horizon-only setup (no legacy/v1 steps)
- Stop/remove standalone tap-agent container if present:
  - `docker ps | grep tap-agent`
  - `docker stop tap-agent && docker rm tap-agent`
- Start unified binary (indexer-service) with `agent.enabled=true` and `agent.tap_agent_enabled=true`:
  - Update config (see below)
  - `cd contrib && docker compose -f docker-compose.yml up -d indexer-service`
- Confirm management API binds to `127.0.0.1`:
  - `curl -s http://127.0.0.1:7601/health`

Execution:
- Run v2/Horizon integration path only (skip any v1 steps)
  - `just test-local-v2`
- Queue an explicit POI action via management API (with public POI + block number)
  - `curl -s -X POST http://127.0.0.1:7601/graphql \`
    `-H "Content-Type: application/json" \`
    `-d '{"query":"mutation Queue($actions:[ActionInput!]!){ queueActions(actions:$actions){ id actionType poi publicPoi poiBlockNumber } }","variables":{"actions":[{"actionType":"UNALLOCATE","deploymentId":"QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY","allocationId":"0x1234567890123456789012345678901234567890","source":"phase4","reason":"explicit poi","protocolNetwork":"eip155:1","poi":"0x0000000000000000000000000000000000000000000000000000000000000001","publicPoi":"0x0000000000000000000000000000000000000000000000000000000000000002","poiBlockNumber":10}]}}'}' | jq`
- Confirm action persists in DB and is executed
  - `docker exec postgres psql -U postgres -d indexer_components_1 -tAc "SELECT id, status, poi, public_poi, poi_block_number, transaction, unallocate_transaction FROM \"Actions\" ORDER BY id DESC LIMIT 1;"`
- Trigger reallocate and verify partial failure logging includes unallocate tx hash
  - `docker logs indexer-service | grep -i "Reallocate partial failure"`
- Simulate outage and restart; verify reconciliation recovers
  - `docker stop indexer-service`
  - `docker start indexer-service`
  - `docker logs indexer-service | grep -i "Reconciliation cycle complete"`
- Validate metrics: `agent_reallocate_partial_failures_total`, invalid input counters
  - `curl -s http://127.0.0.1:7601/metrics | grep -E "agent_reallocate_partial_failures_total|agent_validation_failed_total"`

Evidence capture:
- Save timestamps + logs for: allocation create/close, POI handling, reconciliation recovery
  - `docker logs indexer-service | grep -E "Allocation|POI|Reconciliation" > /tmp/phase4-logs.txt`
- Record action IDs and tx hashes (both unallocate + allocate for reallocate)
  - `docker exec postgres psql -U postgres -d indexer_components_1 -tAc "SELECT id, type, status, transaction, unallocate_transaction FROM \"Actions\" ORDER BY id DESC LIMIT 5;" > /tmp/phase4-actions.txt`
- Note any deviations or operator steps

## Rollback Plan
- If any critical flow fails (allocation/POI/reconcile), stop unified binary and revert to last known-good tag.
- Keep DB intact; do not backfill or edit actions manually except to re-queue safe retries.
- If a reallocate partial failure occurs:
  - Use stored `unallocate_transaction` to verify close succeeded
  - Re-queue allocation with correct amount once safe
  - Record incident and test notes

## Unified Binary Config Snippet
Add to your config (example):
```
[agent]
enabled = true
tap_agent_enabled = true

[agent.management_api]
host = "127.0.0.1"
port = 7601
```
