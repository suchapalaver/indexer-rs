-- Rollback baseline migration
-- WARNING: This will delete ALL data in the affected tables!

--------------------------------------------------------------------------------
-- SECTION 1: Drop DIPS Tables
--------------------------------------------------------------------------------

DROP INDEX IF EXISTS ix_uniq_signature_agreement;
DROP TABLE IF EXISTS indexing_agreements CASCADE;

--------------------------------------------------------------------------------
-- SECTION 2: Drop Horizon (V2) TAP Tables
--------------------------------------------------------------------------------

DROP TRIGGER IF EXISTS deny_update ON tap_horizon_denylist;
DROP FUNCTION IF EXISTS tap_horizon_deny_notify();
DROP TABLE IF EXISTS tap_horizon_denylist CASCADE;

DROP TABLE IF EXISTS tap_horizon_rav_requests_failed CASCADE;
DROP TABLE IF EXISTS tap_horizon_ravs CASCADE;

DROP TRIGGER IF EXISTS receipt_update ON tap_horizon_receipts;
DROP FUNCTION IF EXISTS tap_horizon_receipt_notify();
DROP TABLE IF EXISTS tap_horizon_receipts_invalid CASCADE;
DROP TABLE IF EXISTS tap_horizon_receipts CASCADE;

--------------------------------------------------------------------------------
-- SECTION 3: Drop Cost Models Table
--------------------------------------------------------------------------------

DROP TRIGGER IF EXISTS cost_models_update ON "CostModels";
DROP FUNCTION IF EXISTS cost_models_update_notify();
DROP TABLE IF EXISTS "CostModels" CASCADE;

--------------------------------------------------------------------------------
-- SECTION 4: Drop Agent Tables
--------------------------------------------------------------------------------

DROP TABLE IF EXISTS "POIDisputes" CASCADE;
DROP TABLE IF EXISTS "Actions" CASCADE;
DROP TABLE IF EXISTS "IndexingRules" CASCADE;

-- Drop enum types
DROP TYPE IF EXISTS action_status;
DROP TYPE IF EXISTS action_type;
DROP TYPE IF EXISTS identifier_type;
DROP TYPE IF EXISTS indexing_decision_basis;
