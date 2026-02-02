# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

indexer-rs is a Rust implementation of Graph Protocol indexer services, replacing the TypeScript indexer-service. It consists of two main applications:

- **indexer-service-rs**: Axum-based web server that routes subgraph queries to graph-node and handles TAP receipt processing. Can also run TAP agent in unified binary mode.
- **indexer-tap-agent**: Actor-based system (ractor) that processes TAP receipts into RAVs (Redeemable Aggregate Values)

## Common Commands

The `just` command wraps most development workflows. Run `just help` to see all available targets.

```bash
# Run all CI checks locally (format, clippy, test, sqlx-prepare)
just ci

# Individual commands
just fmt                             # Format code (requires nightly)
just clippy                          # Lint (requires nightly)
just test                            # Run tests with nextest
just review                          # Review snapshot tests

# Run a single test
cargo nextest run test_name
cargo test test_name -- --exact

# Database setup (requires running PostgreSQL)
just psql-up                         # Start PostgreSQL container and run migrations
just migrate                         # Run migrations only

# Update SQLx prepared query metadata (after changing queries)
just sqlx-prepare

# Build release binaries
cargo build --release -p indexer-service-rs
cargo build --release -p indexer-tap-agent
```

## Architecture

### Crate Structure

The workspace contains these crates with the following dependency relationships:

```
indexer-service-rs ─┬─ indexer-config (TOML config via figment)
                    ├─ indexer-monitor ─┬─ indexer-allocation
                    │                   ├─ indexer-query (GraphQL)
                    │                   └─ indexer-watcher (tokio::sync::watch)
                    ├─ indexer-attestation
                    ├─ indexer-receipt (TAP V2/Horizon receipt handling)
                    ├─ indexer-agent (rules engine, reconciliation, executor)
                    └─ indexer-management-api (GraphQL management API)

indexer-tap-agent ──┬─ indexer-config
                    ├─ indexer-receipt
                    └─ ractor (actor framework)

indexer-dips ─────── (WIP: Distributed Indexing Payment System, gRPC)
indexer-profiler ─── (Profiling utilities)
test-assets ──────── (test fixtures: wallets, receipts, allocations)
integration-tests ── (end-to-end tests against local network)
```

### indexer-service-rs Architecture

Middleware-based Axum server where validation/processing happens in composable middleware layers:
- `crates/service/src/middleware/` - Auth, attestation, TAP validation, metrics
- `crates/service/src/routes/` - Route handlers (cost, health, status, request_handler)
- `crates/service/src/tap/checks/` - TAP receipt validation checks

Supports two operational modes:
- **Standalone mode**: Service only, requires separate indexer-tap-agent process
- **Unified binary mode**: Runs TAP agent internally (`tap_agent.rs`)

When agent mode is enabled, exposes a management API for allocation and indexing rule management.

### indexer-tap-agent Architecture

Three-tier actor hierarchy using ractor:
1. **SenderAccountManager** - Top-level supervisor, monitors escrow accounts, routes receipts
2. **SenderAccount** - Per-sender actor tracking receipts, pending RAVs, manages allocations
3. **SenderAllocation** - Per (sender, allocation) tuple, processes receipts into RAVs

Actor implementations in `crates/tap-agent/src/agent/`.

### indexer-agent (Library Crate)

Provides allocation management functionality:
- **Rules Engine** (`crates/indexer-agent/src/rules/`) - Evaluates indexing rules to decide which deployments to allocate
- **Reconciliation** (`crates/indexer-agent/src/reconciliation/`) - Compares desired vs actual allocations, queues actions
- **Executor** (`crates/indexer-agent/src/executor/`) - Executes queued actions (allocate, unallocate, reallocate)
- **Models** (`crates/indexer-agent/src/models/`) - Data types for indexing rules, actions, POI disputes

### TAP Protocol

The codebase uses TAP V2 (Horizon) exclusively:
- **Tables**: `tap_horizon_receipts`, `tap_horizon_ravs` - keyed by collection_id, payer, data_service, service_provider
- Legacy V1 TAP tables are not supported; Horizon (V2) is the only supported TAP mode

## Key Patterns

### Configuration

- TOML-based configuration with figment
- Environment variable overrides: `INDEXER_<SECTION>__<FIELD>` (double underscore for nesting)
- Examples in `crates/config/minimal-config-example.toml` and `maximal-config-example.toml`

### Database

- PostgreSQL with SQLx compile-time query verification
- **Migrations owned by Rust**: `indexer-service-rs` runs migrations automatically at startup (note: README.md states otherwise but is outdated)
- Baseline migration `migrations/20260126000000_baseline.up.sql` consolidates all tables (agent, TAP, DIPS)
- Legacy incremental migrations archived in `migrations/archive/` for reference
- Prepared query metadata in `.sqlx/` directory enables offline compilation
- **Important**: Set `SQLX_OFFLINE=true` when building or running clippy without a database connection. SQLx verifies queries at compile time, so without this flag you'll see "Connection refused" errors. Example:
  ```bash
  SQLX_OFFLINE=true cargo build
  SQLX_OFFLINE=true cargo +nightly clippy --all-features --all-targets -- -D warnings
  ```

### Testing

- Snapshot tests with insta (`cargo insta review`)
- Database tests use testcontainers (PostgreSQL 15)
- Test utilities in `test-assets` crate
- `test` feature gate enables test-only infrastructure in tap-agent

## Development Setup

```bash
# Start local PostgreSQL (for unit tests)
just psql-up

# Full local network setup (contracts, graph-node, etc. for integration tests)
just setup

# Development reload (compiles and mounts host binaries)
just reload-dev

# View logs
just logs-dev

# Integration tests (assumes local network running via 'just setup')
just test-local      # RAV v1 tests
just test-local-v2   # RAV v2 tests

# Stop all services and clean up
just down
```

## Code Style

- Nightly rustfmt with crate-level import granularity
- Group imports: std first, then external crates
- Named string interpolation (not positional)
- Use dotenvy (not dotenv)
