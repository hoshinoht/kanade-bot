-- `/debug ping` test cards (v4 `debug_messages`): written with the claim,
-- bound to the posted message with the attempt, then registered for the run
-- in `delivery_card_runs` so reactions drive its RSVPs. Never a reminder row
-- and never a held target. `cleared_at` is set by `/debug clear_test` after
-- the message was deleted (or found gone). No FK to runs: rows outlive them.
CREATE TABLE debug_cards (
    attempt_id TEXT PRIMARY KEY REFERENCES delivery_attempts (attempt_id),
    run_id     TEXT NOT NULL CHECK (length(run_id) > 0),
    kind       TEXT NOT NULL CHECK (length(kind) BETWEEN 1 AND 32),
    channel_id TEXT NOT NULL,
    message_id TEXT UNIQUE,
    posted_at  TEXT,
    cleared_at TEXT,
    CHECK ((message_id IS NULL) = (posted_at IS NULL)),
    CHECK (cleared_at IS NULL OR message_id IS NOT NULL)
);
CREATE INDEX debug_cards_run ON debug_cards (run_id);
CREATE INDEX debug_cards_channel ON debug_cards (channel_id, posted_at);
