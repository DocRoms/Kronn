-- KT-426 — blocked_reason and blocked_reason_code describe the currently active
-- hold. Older 0.11 development builds cleared the resume checkpoint without
-- clearing those fields, so Working/AwaitingReview/Done rows could still claim
-- to be awaiting a CLI worker acceptance.
--
-- Preserve the only two live-hold shapes:
--   * Blocked;
--   * Interrupted whose exact origin is Blocked (the hold remains resumable).
-- The transition event that originally recorded the block reason remains the
-- immutable audit trail; only the contradictory active projection is repaired.
UPDATE task_executions
SET blocked_from_status = NULL,
    blocked_reason = NULL,
    blocked_reason_code = NULL
WHERE status <> 'Blocked'
  AND NOT (
      status = 'Interrupted'
      AND COALESCE(interrupted_from_status, '') = 'Blocked'
  );
