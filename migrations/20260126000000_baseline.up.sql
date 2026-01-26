-- Baseline migration for indexer-rs
-- This migration consolidates all tables needed for the unified indexer binary.
-- It can be run on both fresh databases and existing databases migrated from TypeScript agent.

--------------------------------------------------------------------------------
-- SECTION 1: Legacy (V1) TAP Tables
-- These tables are preserved for backwards compatibility until V1 code is removed.
-- They will be dropped in a future migration (Phase 8: V1/Legacy Code Removal).
--------------------------------------------------------------------------------

-- Legacy TAP Receipts
CREATE TABLE IF NOT EXISTS scalar_tap_receipts (
    id BIGSERIAL PRIMARY KEY,
    signer_address CHAR(40) NOT NULL,
    signature BYTEA NOT NULL,
    allocation_id CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    nonce NUMERIC(20) NOT NULL,
    value NUMERIC(39) NOT NULL
);

CREATE INDEX IF NOT EXISTS scalar_tap_receipts_allocation_id_idx ON scalar_tap_receipts (allocation_id);
CREATE INDEX IF NOT EXISTS scalar_tap_receipts_timestamp_ns_idx ON scalar_tap_receipts (timestamp_ns);

CREATE OR REPLACE FUNCTION scalar_tap_receipt_notify()
RETURNS trigger AS
$$
BEGIN
    PERFORM pg_notify('scalar_tap_receipt_notification', format('{"id": %s, "allocation_id": "%s", "signer_address": "%s", "timestamp_ns": %s, "value": %s}', NEW.id, NEW.allocation_id, NEW.signer_address, NEW.timestamp_ns, NEW.value));
    RETURN NEW;
END;
$$ LANGUAGE 'plpgsql';

DROP TRIGGER IF EXISTS receipt_update ON scalar_tap_receipts;
CREATE TRIGGER receipt_update AFTER INSERT OR UPDATE
    ON scalar_tap_receipts
    FOR EACH ROW EXECUTE PROCEDURE scalar_tap_receipt_notify();

-- Legacy TAP Invalid Receipts (for debugging)
CREATE TABLE IF NOT EXISTS scalar_tap_receipts_invalid (
    id BIGSERIAL PRIMARY KEY,
    signer_address CHAR(40) NOT NULL,
    signature BYTEA NOT NULL,
    allocation_id CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    nonce NUMERIC(20) NOT NULL,
    value NUMERIC(39) NOT NULL,
    error_log TEXT NOT NULL DEFAULT ''
);

-- Legacy TAP RAVs
CREATE TABLE IF NOT EXISTS scalar_tap_ravs (
    sender_address CHAR(40) NOT NULL,
    signature BYTEA NOT NULL,
    allocation_id CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    value_aggregate NUMERIC(39) NOT NULL,
    last BOOLEAN DEFAULT FALSE NOT NULL,
    final BOOLEAN DEFAULT FALSE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE,
    updated_at TIMESTAMP WITH TIME ZONE,
    PRIMARY KEY (allocation_id, sender_address)
);

-- Legacy TAP Failed RAV Requests (for debugging)
CREATE TABLE IF NOT EXISTS scalar_tap_rav_requests_failed (
    id BIGSERIAL PRIMARY KEY,
    allocation_id CHAR(40) NOT NULL,
    sender_address CHAR(40) NOT NULL,
    expected_rav JSON NOT NULL,
    rav_response JSON NOT NULL,
    reason TEXT NOT NULL
);

-- Legacy TAP Denylist
CREATE TABLE IF NOT EXISTS scalar_tap_denylist (
    sender_address CHAR(40) PRIMARY KEY
);

CREATE OR REPLACE FUNCTION scalar_tap_deny_notify()
RETURNS trigger AS
$$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_notify('scalar_tap_deny_notification', format('{"tg_op": "DELETE", "sender_address": "%s"}', OLD.sender_address));
        RETURN OLD;
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM pg_notify('scalar_tap_deny_notification', format('{"tg_op": "INSERT", "sender_address": "%s"}', NEW.sender_address));
        RETURN NEW;
    ELSE
        PERFORM pg_notify('scalar_tap_deny_notification', format('{"tg_op": "%s", "sender_address": "%s"}', TG_OP, NEW.sender_address));
        RETURN NEW;
    END IF;
END;
$$ LANGUAGE 'plpgsql';

DROP TRIGGER IF EXISTS deny_update ON scalar_tap_denylist;
CREATE TRIGGER deny_update AFTER INSERT OR UPDATE OR DELETE
    ON scalar_tap_denylist
    FOR EACH ROW EXECUTE PROCEDURE scalar_tap_deny_notify();

--------------------------------------------------------------------------------
-- SECTION 2: Agent Tables
-- Tables for the indexer-agent functionality (IndexingRules, Actions, POIDisputes)
--------------------------------------------------------------------------------

-- IndexingDecisionBasis enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'indexing_decision_basis') THEN
        CREATE TYPE indexing_decision_basis AS ENUM ('rules', 'never', 'always', 'offchain');
    END IF;
END$$;

-- IdentifierType enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'identifier_type') THEN
        CREATE TYPE identifier_type AS ENUM ('deployment', 'subgraph', 'group');
    END IF;
END$$;

-- IndexingRules table
CREATE TABLE IF NOT EXISTS "IndexingRules" (
    id SERIAL UNIQUE NOT NULL,
    identifier VARCHAR NOT NULL,
    identifier_type identifier_type DEFAULT 'group',
    allocation_amount DECIMAL,
    allocation_lifetime INTEGER,
    auto_renewal BOOLEAN NOT NULL DEFAULT true,
    parallel_allocations INTEGER,
    max_allocation_percentage FLOAT,
    min_signal DECIMAL,
    max_signal DECIMAL,
    min_stake DECIMAL,
    min_average_query_fees DECIMAL,
    custom VARCHAR,
    decision_basis indexing_decision_basis NOT NULL DEFAULT 'rules',
    require_supported BOOLEAN NOT NULL DEFAULT true,
    safety BOOLEAN NOT NULL DEFAULT true,
    protocol_network VARCHAR NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    PRIMARY KEY (identifier, protocol_network)
);

-- ActionType enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'action_type') THEN
        CREATE TYPE action_type AS ENUM ('allocate', 'unallocate', 'reallocate');
    END IF;
END$$;

-- ActionStatus enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'action_status') THEN
        CREATE TYPE action_status AS ENUM ('queued', 'approved', 'pending', 'deploying', 'success', 'failed', 'canceled');
    END IF;
END$$;

-- Actions table
CREATE TABLE IF NOT EXISTS "Actions" (
    id SERIAL UNIQUE NOT NULL,
    type action_type NOT NULL,
    status action_status NOT NULL DEFAULT 'queued',
    priority INTEGER DEFAULT 0,
    deployment_id VARCHAR NOT NULL,
    allocation_id VARCHAR,
    amount VARCHAR,
    poi VARCHAR,
    force BOOLEAN,
    source VARCHAR NOT NULL,
    reason VARCHAR NOT NULL,
    transaction VARCHAR,
    failure_reason VARCHAR(1000),
    protocol_network VARCHAR(50) NOT NULL,
    is_legacy BOOLEAN NOT NULL DEFAULT true,
    public_poi VARCHAR,
    poi_block_number INTEGER,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    PRIMARY KEY (id, protocol_network)
);

-- POIDisputes table
CREATE TABLE IF NOT EXISTS "POIDisputes" (
    allocation_id VARCHAR UNIQUE NOT NULL,
    subgraph_deployment_id VARCHAR NOT NULL,
    allocation_indexer VARCHAR NOT NULL,
    allocation_amount DECIMAL NOT NULL,
    allocation_proof VARCHAR NOT NULL,
    closed_epoch INTEGER NOT NULL,
    closed_epoch_reference_proof VARCHAR,
    closed_epoch_start_block_hash VARCHAR NOT NULL,
    closed_epoch_start_block_number INTEGER NOT NULL,
    previous_epoch_reference_proof VARCHAR,
    previous_epoch_start_block_hash VARCHAR NOT NULL,
    previous_epoch_start_block_number INTEGER NOT NULL,
    status VARCHAR NOT NULL,
    protocol_network VARCHAR NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    PRIMARY KEY (allocation_id, protocol_network)
);

--------------------------------------------------------------------------------
-- SECTION 3: Cost Models Table
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS "CostModels" (
    id SERIAL,
    deployment VARCHAR NOT NULL,
    model TEXT,
    variables JSONB,
    PRIMARY KEY (deployment)
);

-- Cost model notification function and trigger
CREATE OR REPLACE FUNCTION cost_models_update_notify()
RETURNS trigger AS
$$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_notify('cost_models_update_notification', format('{"tg_op": "DELETE", "deployment": "%s"}', OLD.deployment));
        RETURN OLD;
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM pg_notify('cost_models_update_notification', format('{"tg_op": "INSERT", "deployment": "%s", "model": "%s", "variables": "%s"}', NEW.deployment, NEW.model, NEW.variables));
        RETURN NEW;
    ELSE
        PERFORM pg_notify('cost_models_update_notification', format('{"tg_op": "%s", "deployment": "%s", "model": "%s", "variables": "%s"}', TG_OP, NEW.deployment, NEW.model, NEW.variables));
        RETURN NEW;
    END IF;
END;
$$ LANGUAGE 'plpgsql';

DROP TRIGGER IF EXISTS cost_models_update ON "CostModels";
CREATE TRIGGER cost_models_update AFTER INSERT OR UPDATE OR DELETE
    ON "CostModels"
    FOR EACH ROW EXECUTE PROCEDURE cost_models_update_notify();

--------------------------------------------------------------------------------
-- SECTION 4: Horizon (V2) TAP Tables
--------------------------------------------------------------------------------

-- TAP Horizon Receipts
CREATE TABLE IF NOT EXISTS tap_horizon_receipts (
    id BIGSERIAL PRIMARY KEY,
    signer_address CHAR(40) NOT NULL,
    signature BYTEA NOT NULL,
    collection_id CHAR(64) NOT NULL,
    payer CHAR(40) NOT NULL,
    data_service CHAR(40) NOT NULL,
    service_provider CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    nonce NUMERIC(20) NOT NULL,
    value NUMERIC(39) NOT NULL
);

CREATE INDEX IF NOT EXISTS tap_horizon_receipts_collection_id_idx ON tap_horizon_receipts (collection_id);
CREATE INDEX IF NOT EXISTS tap_horizon_receipts_timestamp_ns_idx ON tap_horizon_receipts (timestamp_ns);

CREATE OR REPLACE FUNCTION tap_horizon_receipt_notify()
RETURNS trigger AS
$$
BEGIN
    PERFORM pg_notify('tap_horizon_receipt_notification', format('{"id": %s, "collection_id": "%s", "signer_address": "%s", "timestamp_ns": %s, "value": %s}', NEW.id, NEW.collection_id, NEW.signer_address, NEW.timestamp_ns, NEW.value));
    RETURN NEW;
END;
$$ LANGUAGE 'plpgsql';

DROP TRIGGER IF EXISTS receipt_update ON tap_horizon_receipts;
CREATE TRIGGER receipt_update AFTER INSERT OR UPDATE
    ON tap_horizon_receipts
    FOR EACH ROW EXECUTE PROCEDURE tap_horizon_receipt_notify();

-- TAP Horizon Invalid Receipts (for debugging)
CREATE TABLE IF NOT EXISTS tap_horizon_receipts_invalid (
    id BIGSERIAL PRIMARY KEY,
    signer_address CHAR(40) NOT NULL,
    signature BYTEA NOT NULL,
    collection_id CHAR(64) NOT NULL,
    payer CHAR(40) NOT NULL,
    data_service CHAR(40) NOT NULL,
    service_provider CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    nonce NUMERIC(20) NOT NULL,
    value NUMERIC(39) NOT NULL,
    error_log TEXT NOT NULL DEFAULT ''
);

-- TAP Horizon RAVs
CREATE TABLE IF NOT EXISTS tap_horizon_ravs (
    signature BYTEA NOT NULL,
    collection_id CHAR(64) NOT NULL,
    payer CHAR(40) NOT NULL,
    data_service CHAR(40) NOT NULL,
    service_provider CHAR(40) NOT NULL,
    timestamp_ns NUMERIC(20) NOT NULL,
    value_aggregate NUMERIC(39) NOT NULL,
    metadata BYTEA NOT NULL,
    last BOOLEAN DEFAULT FALSE NOT NULL,
    final BOOLEAN DEFAULT FALSE NOT NULL,
    redeemed_at TIMESTAMP WITH TIME ZONE,
    created_at TIMESTAMP WITH TIME ZONE,
    updated_at TIMESTAMP WITH TIME ZONE,
    PRIMARY KEY (payer, data_service, service_provider, collection_id)
);

-- TAP Horizon Failed RAV Requests (for debugging)
CREATE TABLE IF NOT EXISTS tap_horizon_rav_requests_failed (
    id BIGSERIAL PRIMARY KEY,
    collection_id CHAR(64) NOT NULL,
    payer CHAR(40) NOT NULL,
    data_service CHAR(40) NOT NULL,
    service_provider CHAR(40) NOT NULL,
    expected_rav JSON NOT NULL,
    rav_response JSON NOT NULL,
    reason TEXT NOT NULL
);

-- TAP Horizon Denylist
CREATE TABLE IF NOT EXISTS tap_horizon_denylist (
    sender_address CHAR(40) PRIMARY KEY
);

CREATE OR REPLACE FUNCTION tap_horizon_deny_notify()
RETURNS trigger AS
$$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM pg_notify('tap_horizon_deny_notification', format('{"tg_op": "DELETE", "sender_address": "%s"}', OLD.sender_address));
        RETURN OLD;
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM pg_notify('tap_horizon_deny_notification', format('{"tg_op": "INSERT", "sender_address": "%s"}', NEW.sender_address));
        RETURN NEW;
    ELSE
        PERFORM pg_notify('tap_horizon_deny_notification', format('{"tg_op": "%s", "sender_address": "%s"}', TG_OP, NEW.sender_address));
        RETURN NEW;
    END IF;
END;
$$ LANGUAGE 'plpgsql';

DROP TRIGGER IF EXISTS deny_update ON tap_horizon_denylist;
CREATE TRIGGER deny_update AFTER INSERT OR UPDATE OR DELETE
    ON tap_horizon_denylist
    FOR EACH ROW EXECUTE PROCEDURE tap_horizon_deny_notify();

--------------------------------------------------------------------------------
-- SECTION 5: DIPS (Distributed Indexing Payment System) Tables
--------------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS indexing_agreements (
    id UUID PRIMARY KEY,
    signature BYTEA NOT NULL,
    signed_payload BYTEA NOT NULL,
    protocol_network VARCHAR(255) NOT NULL,
    chain_id VARCHAR(255) NOT NULL,
    base_price_per_epoch NUMERIC(39) NOT NULL,
    price_per_entity NUMERIC(39) NOT NULL,
    subgraph_deployment_id VARCHAR(255) NOT NULL,
    service CHAR(40) NOT NULL,
    payee CHAR(40) NOT NULL,
    payer CHAR(40) NOT NULL,
    deadline TIMESTAMP WITH TIME ZONE NOT NULL,
    duration_epochs BIGINT NOT NULL,
    max_initial_amount NUMERIC(39) NOT NULL,
    max_ongoing_amount_per_epoch NUMERIC(39) NOT NULL,
    min_epochs_per_collection BIGINT NOT NULL,
    max_epochs_per_collection BIGINT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL,
    cancelled_at TIMESTAMP WITH TIME ZONE,
    signed_cancellation_payload BYTEA,
    current_allocation_id CHAR(40),
    last_allocation_id CHAR(40),
    last_payment_collected_at TIMESTAMP WITH TIME ZONE
);

CREATE UNIQUE INDEX IF NOT EXISTS ix_uniq_signature_agreement ON indexing_agreements(signature);
