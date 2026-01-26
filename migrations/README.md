# Database Migrations

As of the unified indexer-rs binary, database migrations are owned and executed by indexer-service-rs. This replaces the previous model where the TypeScript indexer-agent managed migrations.

## Migration Ownership

- **indexer-service-rs** runs migrations automatically at startup using SQLx
- Migrations are idempotent and safe to run multiple times
- The baseline migration uses `IF NOT EXISTS` and `CREATE OR REPLACE` patterns

## Migration Strategy

The `20260126000000_baseline` migration consolidates all tables:

1. **Agent Tables** - `IndexingRules`, `Actions`, `POIDisputes` for allocation management
2. **Cost Models** - `CostModels` table with pg_notify triggers
3. **TAP Horizon (V2)** - `tap_horizon_receipts`, `tap_horizon_ravs`, `tap_horizon_denylist`
4. **DIPS** - `indexing_agreements` for distributed indexing payment system

### Legacy (V1) TAP Tables

The baseline migration **preserves** Legacy V1 TAP tables for backwards compatibility:
- `scalar_tap_receipts` / `scalar_tap_receipts_invalid`
- `scalar_tap_ravs` / `scalar_tap_rav_requests_failed`
- `scalar_tap_denylist`

These tables will be removed in a future migration once V1 code is fully deprecated.

## Operator Migration Guide

### Fresh Installation

No special steps needed. Start indexer-service-rs and migrations run automatically.

### Upgrading from TypeScript indexer-agent

1. **Stop the TypeScript indexer-agent**

2. **Backup your database**
   ```bash
   pg_dump -h localhost -U postgres indexer_db > backup.sql
   ```

3. **Verify V1 receipts are processed**
   ```sql
   -- Check for unprocessed V1 receipts
   SELECT COUNT(*) FROM scalar_tap_receipts;

   -- Check for unredeemed V1 RAVs
   SELECT COUNT(*) FROM scalar_tap_ravs WHERE final = false;
   ```

   If counts are non-zero, wait for TAP agent to process them before upgrading.

4. **Start indexer-service-rs**

   The service will automatically:
   - Run the baseline migration
   - Create agent tables if they don't exist
   - Preserve Legacy V1 tables (will be removed in a future release)

5. **Verify migration success**
   ```sql
   -- Verify agent tables exist
   SELECT COUNT(*) FROM "IndexingRules";
   SELECT COUNT(*) FROM "Actions";

   -- Verify Horizon tables exist
   SELECT COUNT(*) FROM tap_horizon_receipts;
   ```

### Rollback

If you need to rollback to the TypeScript agent:

```bash
# Rollback migration (WARNING: drops all tables and data!)
sqlx migrate revert --database-url $DATABASE_URL

# Restore from backup
psql -h localhost -U postgres indexer_db < backup.sql
```

## Development

### Prerequisites

Install sqlx-cli:
```bash
cargo install sqlx-cli --no-default-features --features native-tls,postgres
```

### Running Migrations Manually

```bash
export DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5432/indexer
sqlx migrate run --database-url $DATABASE_URL
```

### Creating New Migrations

```bash
sqlx migrate add <migration_name>
```

### Updating SQLx Prepared Queries

After modifying any SQL queries in the codebase:
```bash
cargo sqlx prepare --workspace -- --all-targets --all-features
```

## Archived Migrations

Previous incremental migrations have been archived in `migrations/archive/`. These are preserved for reference but are superseded by the baseline migration.
