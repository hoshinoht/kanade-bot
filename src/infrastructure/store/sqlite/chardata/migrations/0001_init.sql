-- The character-data file (module character_data): its own ledger, never
-- in the main store or its backups. Timestamps are UTC ISO text.

-- Kanade-owned: a roster character opted in to NEXON Open API data.
CREATE TABLE links (
    character_id     TEXT PRIMARY KEY,
    member_id        TEXT NOT NULL,
    ign_key          TEXT NOT NULL,
    region           TEXT NOT NULL CHECK (region = 'msea'),
    status           TEXT NOT NULL CHECK (status IN ('pending', 'active', 'stale')),
    requested_by_kind TEXT NOT NULL CHECK (requested_by_kind IN ('member', 'admin', 'system')),
    requested_by_id  TEXT NOT NULL,
    requested_at     TEXT NOT NULL,
    consent_version  INTEGER CHECK (consent_version > 0),
    consented_at     TEXT,
    linked_at        TEXT,
    version          INTEGER NOT NULL CHECK (version > 0),
    CHECK ((consent_version IS NULL) = (consented_at IS NULL)),
    CHECK (status <> 'pending' OR consent_version IS NULL),
    CHECK (status = 'pending' OR (consent_version IS NOT NULL AND linked_at IS NOT NULL))
) WITHOUT ROWID;

-- NEXON data: at most 30 days from the older fetch, gone with its link.
CREATE TABLE snapshots (
    character_id    TEXT PRIMARY KEY REFERENCES links (character_id) ON DELETE CASCADE,
    ocid            TEXT NOT NULL CHECK (length(ocid) BETWEEN 1 AND 128),
    ocid_fetched_at TEXT NOT NULL,
    character_name  TEXT CHECK (length(character_name) <= 64),
    world_name      TEXT CHECK (length(world_name) <= 64),
    character_class TEXT CHECK (length(character_class) <= 64),
    level           INTEGER CHECK (level BETWEEN 0 AND 65535),
    combat_power    INTEGER CHECK (combat_power >= 0),
    image_url       TEXT CHECK (length(image_url) <= 1024),
    data_date       TEXT,
    fetched_at      TEXT NOT NULL,
    expires_at      TEXT NOT NULL
);

CREATE TABLE refresh_state (
    character_id    TEXT PRIMARY KEY REFERENCES links (character_id) ON DELETE CASCADE,
    next_attempt_at TEXT,
    last_error_code TEXT CHECK (length(last_error_code) <= 32)
);

-- Persistent Discord posts showing NEXON data; written before the send,
-- deleted once stripped. Kept past an unlink until then (no FK).
CREATE TABLE posts (
    ref          TEXT PRIMARY KEY CHECK (length(ref) = 6),
    channel_id   TEXT NOT NULL,
    message_id   TEXT,
    character_id TEXT NOT NULL,
    posted_at    TEXT NOT NULL,
    strip_due_at TEXT NOT NULL,
    state        TEXT NOT NULL CHECK (state IN ('live', 'stripping', 'unknown', 'stuck'))
);
CREATE INDEX posts_character ON posts (character_id);

-- The rolling 24-hour call budget; survives restarts.
CREATE TABLE calls (
    id INTEGER PRIMARY KEY,
    at TEXT NOT NULL
);
CREATE INDEX calls_at ON calls (at);
