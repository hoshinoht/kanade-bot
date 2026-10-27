-- Durable decline candidates and their v4 delivery bindings.  A candidate is
-- recorded with the RSVP commit; the message binding is intentionally nullable
-- until Discord confirms the create.
ALTER TABLE decline_notices ADD COLUMN reference_id TEXT;
ALTER TABLE decline_notices ADD COLUMN display_name TEXT
    CHECK (display_name IS NULL OR length(display_name) <= 128);
ALTER TABLE decline_notices ADD COLUMN retract_pending INTEGER NOT NULL DEFAULT 0
    CHECK (retract_pending IN (0, 1));
CREATE INDEX decline_notices_pending ON decline_notices (notified_at, run_id, user_id)
WHERE message_id IS NULL OR retract_pending = 1;
