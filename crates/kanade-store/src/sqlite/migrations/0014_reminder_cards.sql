-- What a reminder card was posted as, written before its claim and keyed by
-- the send's native dedupe key: a retry and later edits reuse it. `heading`
-- is the day-of heading line (a persona rewrite or v4's text). Written once.
CREATE TABLE reminder_cards (
    dedupe_key TEXT PRIMARY KEY CHECK (
        length(dedupe_key) = 64 AND dedupe_key NOT GLOB '*[^0-9a-f]*'
    ),
    kind       TEXT NOT NULL CHECK (kind = 'day_of' OR kind GLOB 'countdown_*'),
    heading    TEXT CHECK (heading IS NULL OR kind = 'day_of'),
    created_at TEXT NOT NULL
);

CREATE TRIGGER reminder_cards_no_update
BEFORE UPDATE ON reminder_cards
BEGIN
    SELECT RAISE(ABORT, 'reminder cards are written once');
END;

-- A reaction refreshes every posted card of its runs.
CREATE INDEX delivery_card_runs_run ON delivery_card_runs (run_id);
