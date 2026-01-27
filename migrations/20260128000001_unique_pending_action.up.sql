-- Ensure at most one non-terminal action per deployment per network.
-- This enforces Invariant 8.1: No Duplicate Pending Actions.
--
-- A partial unique index only applies to non-terminal statuses (queued, approved,
-- pending, deploying). Terminal statuses (success, failed, canceled) are excluded,
-- allowing historical actions to accumulate without constraint violations.

CREATE UNIQUE INDEX idx_one_pending_action_per_deployment
ON "Actions" (deployment_id, protocol_network)
WHERE status IN ('queued', 'approved', 'pending', 'deploying');

COMMENT ON INDEX idx_one_pending_action_per_deployment IS
    'Ensures at most one active action per deployment. Terminal statuses (success, failed, canceled) are excluded.';
