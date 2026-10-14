-- Idempotency-Key replays for admin effects that are not scheduler commits
-- (Limits window clear, manual digest and header rewrite, rescan submit and
-- cancel, Config PATCH). One row per (scope, actor, key): the SHA-256 of the
-- normalized full request, the answer to replay and a bounded expiry. An
-- expired row reads as absent; every replay write deletes expired rows first.
CREATE TABLE idempotency_replays (
    scope      TEXT NOT NULL CHECK (scope IN ('limits', 'rescan', 'config')),
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

CREATE INDEX idempotency_replays_expiry ON idempotency_replays (expires_at);
