#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

echo "== Phase 4: Local-network integration (Horizon-only) =="

echo "== Pre-flight: clean slate =="
just down

echo "== Pre-flight: setup local-network =="
./setup-test-network.sh

echo "== Pre-flight: stop standalone tap-agent (unified binary only) =="
if docker ps | grep -q "tap-agent"; then
  docker stop tap-agent || true
  docker rm tap-agent || true
fi

echo "== Start unified indexer-service =="
cd contrib
docker compose -f docker-compose.yml up -d indexer-service
cd "$ROOT_DIR"

echo "== Health check =="
curl -fsS http://127.0.0.1:7601/health >/dev/null

echo "== V2/Horizon integration path =="
just test-local-v2

echo "== Queue explicit POI action via management API =="
curl -s -X POST http://127.0.0.1:7601/graphql \
  -H "Content-Type: application/json" \
  -d '{"query":"mutation Queue($actions:[ActionInput!]!){ queueActions(actions:$actions){ id actionType poi publicPoi poiBlockNumber } }","variables":{"actions":[{"actionType":"UNALLOCATE","deploymentId":"QmSWxvd8SaQK6qZKJ7xtfxCCGoRzGnoi2WNzmJYYJW9BXY","allocationId":"0x1234567890123456789012345678901234567890","source":"phase4","reason":"explicit poi","protocolNetwork":"eip155:1","poi":"0x0000000000000000000000000000000000000000000000000000000000000001","publicPoi":"0x0000000000000000000000000000000000000000000000000000000000000002","poiBlockNumber":10}]}}'}' | jq

echo "== Verify POI fields persisted =="
docker exec postgres psql -U postgres -d indexer_components_1 -tAc \
  "SELECT id, status, poi, public_poi, poi_block_number, transaction, unallocate_transaction FROM \"Actions\" ORDER BY id DESC LIMIT 1;"

echo "== Check for reallocate partial failure logs (if any) =="
docker logs indexer-service | grep -i "Reallocate partial failure" || true

echo "== Simulate outage and recovery =="
docker stop indexer-service
docker start indexer-service
docker logs indexer-service | grep -i "Reconciliation cycle complete" || true

echo "== Metrics sanity check =="
curl -s http://127.0.0.1:7601/metrics | grep -E "agent_reallocate_partial_failures_total|agent_validation_failed_total" || true

echo "== Evidence capture =="
docker logs indexer-service | grep -E "Allocation|POI|Reconciliation" > /tmp/phase4-logs.txt
docker exec postgres psql -U postgres -d indexer_components_1 -tAc \
  "SELECT id, type, status, transaction, unallocate_transaction FROM \"Actions\" ORDER BY id DESC LIMIT 5;" \
  > /tmp/phase4-actions.txt

echo "== Phase 4 execution complete =="
