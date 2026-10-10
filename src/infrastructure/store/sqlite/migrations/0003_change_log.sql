-- The append-only, hash-chained schedule change history (domain::history).
-- `body` is the record's canonical JSON; `hash` is its SHA-256. The other
-- columns index the body for queries and are checked against it on
-- verification. Records are never updated or deleted.
CREATE TABLE change_log (
    seq        INTEGER PRIMARY KEY CHECK (seq >= 0),
    id         TEXT NOT NULL UNIQUE,
    revision   INTEGER NOT NULL CHECK (revision >= 0),
    at         TEXT NOT NULL,
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('member', 'admin', 'system')),
    actor_id   TEXT NOT NULL,
    surface    TEXT NOT NULL CHECK (surface IN (
        'discord', 'admin_portal', 'public_portal', 'cli', 'chat_approval',
        'extraction_approval', 'delivery_tick', 'rollback', 'import',
        'draft_merge', 'request_merge', 'cherry_pick'
    )),
    request_id TEXT,
    -- Digest of the request that used `request_id` (idempotency); stored
    -- beside the record, not part of its hashed body.
    request_digest TEXT,
    body       TEXT NOT NULL,
    prev_hash  TEXT NOT NULL CHECK (
        length(prev_hash) = 64 AND prev_hash NOT GLOB '*[^0-9a-f]*'
    ),
    hash       TEXT NOT NULL UNIQUE CHECK (
        length(hash) = 64 AND hash NOT GLOB '*[^0-9a-f]*'
    )
);
CREATE INDEX change_log_actor ON change_log (actor_kind, actor_id, seq);
CREATE INDEX change_log_revision ON change_log (revision);
-- Idempotency: one record per actor and request id.
CREATE UNIQUE INDEX change_log_request
    ON change_log (actor_kind, actor_id, request_id) WHERE request_id IS NOT NULL;

CREATE TABLE change_log_weeks (
    seq        INTEGER NOT NULL REFERENCES change_log (seq),
    week_start TEXT NOT NULL,
    PRIMARY KEY (week_start, seq)
);

CREATE TRIGGER change_log_no_update BEFORE UPDATE ON change_log
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
CREATE TRIGGER change_log_no_delete BEFORE DELETE ON change_log
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
CREATE TRIGGER change_log_weeks_no_update BEFORE UPDATE ON change_log_weeks
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
CREATE TRIGGER change_log_weeks_no_delete BEFORE DELETE ON change_log_weeks
BEGIN
    SELECT RAISE(ABORT, 'change history is append-only');
END;
