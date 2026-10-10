-- Run completion prompts (user decisions 2026-10-09/10): half an hour after a
-- run ends Kanade asks its channel whether it happened. One row per ask: an
-- ask closes once (Done, Didn't happen, Not yet, automatic done at the
-- cutoff, the run moved, or it ended another way); "Not yet" and a move open
-- the next ask. At most one ask per run is open.
CREATE TABLE run_prompts (
    run_id     TEXT NOT NULL CHECK (length(run_id) BETWEEN 1 AND 64),
    ask        INTEGER NOT NULL CHECK (ask >= 0),
    ends_at    TEXT NOT NULL,
    due_at     TEXT NOT NULL,
    cutoff_at  TEXT NOT NULL CHECK (cutoff_at > due_at),
    channel_id TEXT CHECK (channel_id IS NULL OR length(channel_id) BETWEEN 1 AND 32),
    message_id TEXT CHECK (message_id IS NULL OR length(message_id) BETWEEN 1 AND 32),
    outcome    TEXT CHECK (outcome IS NULL OR outcome IN (
        'done', 'didnt_happen', 'not_yet', 'auto_done', 'moved', 'closed'
    )),
    -- The presser's user id, for a pressed outcome only.
    decided_by TEXT CHECK (decided_by IS NULL OR length(decided_by) BETWEEN 1 AND 32),
    decided_at TEXT,
    -- Its posted message was edited to show the outcome (or is gone).
    message_settled INTEGER NOT NULL DEFAULT 0 CHECK (message_settled IN (0, 1)),
    PRIMARY KEY (run_id, ask),
    CHECK ((outcome IS NULL) = (decided_at IS NULL)),
    CHECK (coalesce(outcome IN ('done', 'didnt_happen', 'not_yet'), 0)
        = (decided_by IS NOT NULL))
);

-- One open ask per run.
CREATE UNIQUE INDEX run_prompts_one_open ON run_prompts (run_id) WHERE outcome IS NULL;
CREATE INDEX run_prompts_unsettled ON run_prompts (decided_at)
    WHERE outcome IS NOT NULL AND message_id IS NOT NULL AND message_settled = 0;

-- A closed ask never reopens or changes its outcome.
CREATE TRIGGER run_prompts_close_once BEFORE UPDATE OF outcome, decided_by, decided_at
ON run_prompts
WHEN OLD.outcome IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'run prompt already closed');
END;
