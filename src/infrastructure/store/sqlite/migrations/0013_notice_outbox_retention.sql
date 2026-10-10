-- Why a notice left the outbox: 'journal' (claimed and resolved through the
-- delivery journal), 'stale' (too old when the drain reached it) or 'silent'
-- (nothing left to post, e.g. its run is gone); the last two were never
-- claimed or posted. Rows drained before this migration keep NULL.
ALTER TABLE notice_outbox ADD COLUMN drained_reason TEXT
    CHECK (drained_reason IS NULL OR
           (state = 'drained' AND drained_reason IN ('journal', 'stale', 'silent')));

-- Pending notices are never deleted. Drained ones go only through the log
-- retention purge (`prune_model_logs`, drained before the 90-day cutoff); the
-- age is enforced there because a trigger cannot see the injected clock.
CREATE TRIGGER notice_outbox_keep_pending BEFORE DELETE ON notice_outbox
WHEN OLD.state IS NOT 'drained' OR OLD.drained_at IS NULL
BEGIN SELECT RAISE(ABORT, 'a pending outbox notice cannot be deleted'); END;

CREATE INDEX notice_outbox_drained ON notice_outbox (drained_at, id)
    WHERE state = 'drained';
