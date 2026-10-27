-- Blame index and checkpoints (domain::history::{blame, checkpoint}).

-- Which record set which field of which run or weekly timing; written in the
-- same transaction as the record (derived from its rows) and checked against
-- the records by history verification.
CREATE TABLE change_fields (
    target_kind TEXT NOT NULL CHECK (target_kind IN ('run', 'fixed_run')),
    target_id   TEXT NOT NULL,
    field       TEXT NOT NULL,
    seq         INTEGER NOT NULL REFERENCES change_log (seq),
    PRIMARY KEY (target_kind, target_id, field, seq)
);
CREATE INDEX change_fields_seq ON change_fields (seq);

-- Immutable named pointers at the history head.
CREATE TABLE checkpoints (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    kind            TEXT NOT NULL CHECK (kind IN ('auto', 'admin')),
    seq             INTEGER NOT NULL REFERENCES change_log (seq),
    hash            TEXT NOT NULL CHECK (length(hash) = 64 AND hash NOT GLOB '*[^0-9a-f]*'),
    revision        INTEGER NOT NULL CHECK (revision >= 0),
    week_start      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    created_by_kind TEXT NOT NULL CHECK (created_by_kind IN ('member', 'admin', 'system')),
    created_by_id   TEXT NOT NULL
);
-- One automatic checkpoint per boss week.
CREATE UNIQUE INDEX checkpoints_auto_week ON checkpoints (week_start) WHERE kind = 'auto';
CREATE INDEX checkpoints_week ON checkpoints (week_start, id);

CREATE TRIGGER change_fields_no_update BEFORE UPDATE ON change_fields
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
CREATE TRIGGER change_fields_no_delete BEFORE DELETE ON change_fields
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
CREATE TRIGGER checkpoints_no_update BEFORE UPDATE ON checkpoints
BEGIN
    SELECT RAISE(ABORT, 'checkpoints are immutable');
END;
CREATE TRIGGER checkpoints_no_delete BEFORE DELETE ON checkpoints
BEGIN
    SELECT RAISE(ABORT, 'checkpoints are immutable');
END;
