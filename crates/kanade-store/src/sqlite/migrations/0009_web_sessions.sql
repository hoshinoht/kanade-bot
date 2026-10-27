-- Web sessions (infrastructure::store::web_sessions). The primary key is the
-- SHA-256 of the cookie value; the cookie itself is never stored. Instants
-- are UTC ISO text. `public` is reserved for member sessions.

CREATE TABLE web_sessions (
    id_hash      TEXT PRIMARY KEY CHECK (length(id_hash) = 64 AND id_hash NOT GLOB '*[^0-9a-f]*'),
    origin       TEXT NOT NULL CHECK (origin IN ('admin', 'public')),
    method       TEXT NOT NULL CHECK (method IN ('discord', 'tailscale', 'token')),
    subject      TEXT NOT NULL CHECK (length(subject) BETWEEN 1 AND 320),
    display      TEXT NOT NULL CHECK (length(display) <= 200),
    created_at   TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    checked_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL
);
-- Pruning scans: the table holds a handful of live sessions.
CREATE INDEX web_sessions_subject ON web_sessions (origin, method, subject);
