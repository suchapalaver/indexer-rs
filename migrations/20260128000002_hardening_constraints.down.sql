-- Rollback hardening constraints for Actions table

ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_transaction_only_on_success;
ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_allocation_id_required;
ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_amount_required;
