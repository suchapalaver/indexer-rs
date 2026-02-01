-- Rollback hardening constraints for Actions table

ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_transaction_on_terminal_or_deploying;
ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_allocation_id_required;
ALTER TABLE "Actions" DROP CONSTRAINT IF EXISTS chk_amount_required;
