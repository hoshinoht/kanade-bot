-- Drafts: staged schedule operations an administrator reviews and merges
-- (domain::drafts). `base_seq`/`base_hash` is the history head drafted on.
-- Staged operations keep their position (`ord`); the log is append-only.
-- `draft_requests` carries create-draft request ids (idempotency).

CREATE TABLE drafts (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('admin', 'request')),
    title         TEXT NOT NULL CHECK (length(title) >= 1 AND length(title) <= 200),
    author_kind   TEXT NOT NULL CHECK (author_kind IN ('member', 'admin', 'system')),
    author_id     TEXT NOT NULL,
    base_seq      INTEGER NOT NULL CHECK (base_seq >= 0),
    base_hash     TEXT NOT NULL CHECK (
        length(base_hash) = 64 AND base_hash NOT GLOB '*[^0-9a-f]*'
    ),
    base_revision INTEGER NOT NULL CHECK (base_revision >= 0),
    version       INTEGER NOT NULL CHECK (version >= 1),
    status        TEXT NOT NULL CHECK (status IN (
        'open', 'submitted', 'merged', 'discarded', 'rejected', 'withdrawn', 'expired'
    )),
    request_type  TEXT,
    subject       TEXT,
    merged_seq    INTEGER REFERENCES change_log (seq),
    closed_by_kind TEXT CHECK (closed_by_kind IN ('member', 'admin', 'system')),
    closed_by_id  TEXT,
    close_reason  TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    -- The boss week a week-scoped draft expires after the reset passes; NULL
    -- is a weekly-timings-only draft, which never expires by week.
    expires_week  TEXT,
    CHECK ((closed_by_kind IS NULL) = (closed_by_id IS NULL)),
    -- Only a merged draft names its record, and every closed draft names
    -- who closed it (expiry is closed by the system delivery actor).
    CHECK ((status = 'merged') = (merged_seq IS NOT NULL)),
    CHECK ((
        status IN ('merged', 'discarded', 'rejected', 'withdrawn', 'expired')
    ) = (
        closed_by_kind IS NOT NULL AND closed_by_id IS NOT NULL
    ))
);
CREATE INDEX drafts_status ON drafts (status, id);

CREATE TABLE draft_ops (
    draft_id TEXT NOT NULL REFERENCES drafts (id),
    ord      INTEGER NOT NULL CHECK (ord >= 0),
    op       TEXT NOT NULL,
    author_kind TEXT NOT NULL CHECK (author_kind IN ('member', 'admin', 'system')),
    author_id TEXT NOT NULL,
    added_at TEXT NOT NULL,
    PRIMARY KEY (draft_id, ord)
);

CREATE TABLE draft_events (
    id       INTEGER PRIMARY KEY,
    draft_id TEXT NOT NULL REFERENCES drafts (id),
    version  INTEGER NOT NULL CHECK (version >= 1),
    kind     TEXT NOT NULL CHECK (kind IN (
        'created', 'op_added', 'op_edited', 'op_removed', 'rebased', 'merged',
        'discarded', 'expired', 'submitted', 'rejected', 'withdrawn'
    )),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('member', 'admin', 'system')),
    actor_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    detail   TEXT
);
CREATE INDEX draft_events_draft ON draft_events (draft_id, id);

CREATE TRIGGER draft_events_no_update BEFORE UPDATE ON draft_events
BEGIN
    SELECT RAISE(ABORT, 'draft events are append-only');
END;
CREATE TRIGGER draft_events_no_delete BEFORE DELETE ON draft_events
BEGIN
    SELECT RAISE(ABORT, 'draft events are append-only');
END;

-- One row per author and create-draft request id.
CREATE TABLE draft_requests (
    id          INTEGER PRIMARY KEY,
    actor_kind  TEXT NOT NULL CHECK (actor_kind IN ('member', 'admin', 'system')),
    actor_id    TEXT NOT NULL,
    request_id  TEXT NOT NULL,
    digest      TEXT NOT NULL,
    draft_id    TEXT NOT NULL REFERENCES drafts (id),
    recorded_at TEXT NOT NULL,
    UNIQUE (actor_kind, actor_id, request_id)
);
