-- Admin drafts over the API.
--
-- 1. Draft event kind `retitled`: an administrator renamed a draft. SQLite
--    cannot relax a CHECK in place, so `draft_events` is rebuilt under a new
--    name, the old table dropped and the new one renamed; its index and
--    append-only triggers are recreated exactly as 0005 left them.
CREATE TABLE draft_events_v37 (
    id       INTEGER PRIMARY KEY,
    draft_id TEXT NOT NULL REFERENCES drafts (id),
    version  INTEGER NOT NULL CHECK (version >= 1),
    kind     TEXT NOT NULL CHECK (kind IN (
        'created', 'op_added', 'op_edited', 'op_removed', 'rebased', 'merged',
        'discarded', 'expired', 'submitted', 'rejected', 'withdrawn', 'retitled'
    )),
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('member', 'admin', 'system')),
    actor_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    detail   TEXT
);
INSERT INTO draft_events_v37 (id, draft_id, version, kind, actor_kind, actor_id, at, detail)
SELECT id, draft_id, version, kind, actor_kind, actor_id, at, detail FROM draft_events;
DROP TABLE draft_events;
ALTER TABLE draft_events_v37 RENAME TO draft_events;

CREATE INDEX draft_events_draft ON draft_events (draft_id, id);
CREATE TRIGGER draft_events_no_update BEFORE UPDATE ON draft_events
BEGIN
    SELECT RAISE(ABORT, 'draft events are append-only');
END;
CREATE TRIGGER draft_events_no_delete BEFORE DELETE ON draft_events
BEGIN
    SELECT RAISE(ABORT, 'draft events are append-only');
END;

-- 2. Idempotency-Key scope `drafts`: the admin draft writes (create, rename,
--    stage, remove, rebase, discard, merge) replay their stored answers.
--    Rebuilt the same way; the index is recreated as 0031 left it.
CREATE TABLE idempotency_replays_v37 (
    scope      TEXT NOT NULL CHECK (scope IN ('limits', 'rescan', 'config', 'drafts')),
    actor      TEXT NOT NULL CHECK (length(actor) BETWEEN 1 AND 256),
    key        TEXT NOT NULL CHECK (length(key) BETWEEN 1 AND 128),
    request    TEXT NOT NULL
               CHECK (length(request) = 64 AND request NOT GLOB '*[^0-9a-f]*'),
    status     INTEGER NOT NULL CHECK (status BETWEEN 200 AND 299),
    body       TEXT NOT NULL CHECK (length(body) <= 16384 AND json_valid(body)),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL CHECK (expires_at > created_at),
    PRIMARY KEY (scope, actor, key)
) WITHOUT ROWID;
INSERT INTO idempotency_replays_v37
    (scope, actor, key, request, status, body, created_at, expires_at)
SELECT scope, actor, key, request, status, body, created_at, expires_at
FROM idempotency_replays;
DROP TABLE idempotency_replays;
ALTER TABLE idempotency_replays_v37 RENAME TO idempotency_replays;

CREATE INDEX idempotency_replays_expiry ON idempotency_replays (expires_at);
