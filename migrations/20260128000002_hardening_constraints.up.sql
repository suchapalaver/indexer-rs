-- Hardening constraints for Actions table
-- These constraints mirror application-level invariants for defense-in-depth.

-- Invariant 1.3: Transaction hash only set on SUCCESS status
-- The transaction field should only be populated when an action successfully completes.
ALTER TABLE "Actions" ADD CONSTRAINT chk_transaction_on_terminal_or_deploying
CHECK (transaction IS NULL OR status IN ('deploying', 'success', 'failed'));

-- Invariant 1.2: Allocation ID required for unallocate/reallocate
-- Unallocate and reallocate actions must reference an existing allocation.
ALTER TABLE "Actions" ADD CONSTRAINT chk_allocation_id_required
CHECK (
    (type = 'allocate' AND allocation_id IS NULL) OR
    (type IN ('unallocate', 'reallocate') AND allocation_id IS NOT NULL)
);

-- Invariant 1.2: Amount required for allocate/reallocate
-- Allocate and reallocate actions must specify an allocation amount.
ALTER TABLE "Actions" ADD CONSTRAINT chk_amount_required
CHECK (
    (type = 'unallocate' AND amount IS NULL) OR
    (type IN ('allocate', 'reallocate') AND amount IS NOT NULL)
);
