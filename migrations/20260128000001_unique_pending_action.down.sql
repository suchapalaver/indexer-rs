-- Remove the partial unique index for pending actions.
DROP INDEX IF EXISTS idx_one_pending_action_per_deployment;
