-- Guide publisher (module `guides`): admin-written boss guides published to
-- one Discord channel each as an ordered set of messages ("slots"). Outside
-- the history chain (admin writes and run outcomes are audited in
-- `settings_changes`, section `guides`), never sent to a model, no
-- retention. The tables exist whether or not the module is on. Discord ids
-- are 17-20 digit snowflakes; hashes are 64 lowercase hex.
CREATE TABLE guides (
    id         TEXT PRIMARY KEY CHECK (length(id) = 36),
    title      TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 100),
    -- Unique across guides; NULL until an admin picks one.
    channel_id TEXT CHECK (channel_id IS NULL OR (length(channel_id) BETWEEN 17 AND 20
        AND channel_id NOT GLOB '*[^0-9]*')),
    -- The draft: a JSON array of blocks, at most 64 KiB.
    blocks     TEXT NOT NULL CHECK (json_valid(blocks) AND json_type(blocks) = 'array'
        AND length(CAST(blocks AS BLOB)) <= 65536),
    version    INTEGER NOT NULL CHECK (version >= 1),
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL CHECK (length(created_by) BETWEEN 1 AND 320),
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL CHECK (length(updated_by) BETWEEN 1 AND 320)
);

CREATE UNIQUE INDEX guides_channel ON guides (channel_id) WHERE channel_id IS NOT NULL;
CREATE INDEX guides_title ON guides (title, id);

-- Publish runs (and unpublish and guide-wide search runs): at most one
-- queued or running per guide; a finished run never changes.
CREATE TABLE guide_publishes (
    id                  TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 64),
    guide_id            TEXT NOT NULL REFERENCES guides (id) ON DELETE CASCADE,
    version             INTEGER NOT NULL CHECK (version >= 1),
    plan_hash           TEXT NOT NULL CHECK (length(plan_hash) = 64
        AND plan_hash NOT GLOB '*[^0-9a-f]*'),
    channel_id          TEXT NOT NULL CHECK (length(channel_id) BETWEEN 17 AND 20
        AND channel_id NOT GLOB '*[^0-9]*'),
    kind                TEXT NOT NULL CHECK (kind IN ('publish', 'unpublish', 'search')),
    -- The admin's confirmations plus `delete_old`.
    confirmed           TEXT NOT NULL CHECK (json_valid(confirmed)
        AND json_type(confirmed) = 'object'),
    state               TEXT NOT NULL CHECK (state IN ('queued', 'running', 'done', 'stopped',
        'cancelled')),
    stop_reason         TEXT CHECK (stop_reason IS NULL OR stop_reason IN ('plan_changed',
        'unknown_slot', 'edit_unconfirmed', 'delete_unconfirmed', 'rejected',
        'discord_unavailable', 'interrupted')),
    requested_by        TEXT NOT NULL CHECK (length(requested_by) BETWEEN 1 AND 320),
    requested_at        TEXT NOT NULL,
    started_at          TEXT,
    finished_at         TEXT,
    cancel_requested_at TEXT,
    CHECK ((state = 'stopped') = (stop_reason IS NOT NULL)),
    CHECK ((state IN ('done', 'stopped', 'cancelled')) = (finished_at IS NOT NULL)),
    CHECK (state <> 'queued' OR started_at IS NULL),
    CHECK (state NOT IN ('running', 'done') OR started_at IS NOT NULL)
);

CREATE UNIQUE INDEX guide_publishes_live ON guide_publishes (guide_id)
    WHERE state IN ('queued', 'running');
CREATE INDEX guide_publishes_guide ON guide_publishes (guide_id, requested_at);
CREATE INDEX guide_publishes_queued ON guide_publishes (requested_at) WHERE state = 'queued';

CREATE TRIGGER guide_publishes_final BEFORE UPDATE ON guide_publishes
WHEN OLD.state IN ('done', 'stopped', 'cancelled')
BEGIN
    SELECT RAISE(ABORT, 'a finished guide publish never changes');
END;

-- The published slot map. A slot is `(channel, position)`, so during a
-- channel move the new set and the old set coexist until the old messages
-- are deleted, and an interrupted move keeps the old message ids. A row
-- changes only after Discord confirmed an effect or proved it gone.
CREATE TABLE guide_slots (
    guide_id   TEXT NOT NULL REFERENCES guides (id) ON DELETE CASCADE,
    channel_id TEXT NOT NULL CHECK (length(channel_id) BETWEEN 17 AND 20
        AND channel_id NOT GLOB '*[^0-9]*'),
    position   INTEGER NOT NULL CHECK (position BETWEEN 0 AND 199),
    -- Set when posted, kept on a gone slot when known, NULL when unknown.
    message_id TEXT CHECK (message_id IS NULL OR (length(message_id) BETWEEN 17 AND 20
        AND message_id NOT GLOB '*[^0-9]*')),
    hash       TEXT NOT NULL CHECK (length(hash) = 64 AND hash NOT GLOB '*[^0-9a-f]*'),
    -- `[{file_name, sha256}]`, the files the message carries.
    uploads    TEXT NOT NULL CHECK (json_valid(uploads) AND json_type(uploads) = 'array'),
    state      TEXT NOT NULL CHECK (state IN ('posted', 'gone', 'unknown')),
    -- An unknown slot's journal source (`guide-post:<op uuid>`).
    source     TEXT CHECK (source IS NULL OR length(source) BETWEEN 1 AND 128),
    publish_id TEXT NOT NULL REFERENCES guide_publishes (id),
    updated_at TEXT NOT NULL,
    CHECK (state <> 'posted' OR message_id IS NOT NULL),
    CHECK (state <> 'unknown' OR (message_id IS NULL AND source IS NOT NULL)),
    PRIMARY KEY (guide_id, channel_id, position)
);

CREATE INDEX guide_slots_publish ON guide_slots (publish_id);

-- A run's op log, each op recorded `pending` before its effect is tried and
-- settled once. `detail` holds what a settled op found, e.g. the candidate
-- messages of a create that could not be confirmed.
CREATE TABLE guide_publish_ops (
    publish_id TEXT NOT NULL REFERENCES guide_publishes (id) ON DELETE CASCADE,
    ordinal    INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 999),
    kind       TEXT NOT NULL CHECK (kind IN ('edit', 'create', 'delete', 'forget')),
    position   INTEGER NOT NULL CHECK (position BETWEEN 0 AND 199),
    channel_id TEXT NOT NULL CHECK (length(channel_id) BETWEEN 17 AND 20
        AND channel_id NOT GLOB '*[^0-9]*'),
    message_id TEXT CHECK ((kind IN ('edit', 'delete')) = (message_id IS NOT NULL)),
    hash       TEXT CHECK ((kind IN ('edit', 'create')) = (hash IS NOT NULL)
        AND (hash IS NULL OR (length(hash) = 64 AND hash NOT GLOB '*[^0-9a-f]*'))),
    source     TEXT CHECK ((kind = 'create') = (source IS NOT NULL)
        AND (source IS NULL OR length(source) BETWEEN 1 AND 128)),
    outcome    TEXT NOT NULL CHECK (outcome IN ('pending', 'done', 'gone', 'recovered',
        'unknown', 'rejected', 'skipped')),
    detail     TEXT CHECK (detail IS NULL OR (json_valid(detail)
        AND json_type(detail) = 'object')),
    at         TEXT NOT NULL,
    PRIMARY KEY (publish_id, ordinal)
);

CREATE TRIGGER guide_publish_ops_settled BEFORE UPDATE ON guide_publish_ops
WHEN OLD.outcome <> 'pending'
BEGIN
    SELECT RAISE(ABORT, 'a settled guide publish op never changes');
END;
