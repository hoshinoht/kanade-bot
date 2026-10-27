-- v5 initial schema. Instants are canonical UTC ISO text (domain::time::to_iso);
-- lists are JSON arrays of strings. Shapes follow v4 schema v16 unless noted.

CREATE TABLE store_meta (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    -- Advanced by every schedule commit; optimistic concurrency token.
    revision       INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    guild_id       TEXT
);
INSERT INTO store_meta (id, schema_version, revision) VALUES (1, 1, 0);

CREATE TABLE members (
    user_id      TEXT PRIMARY KEY,
    display_name TEXT,
    nickname     TEXT,
    has_role     INTEGER NOT NULL DEFAULT 0 CHECK (has_role IN (0, 1)),
    is_bot       INTEGER NOT NULL DEFAULT 0 CHECK (is_bot IN (0, 1)),
    ping_level   TEXT NOT NULL DEFAULT 'essential'
                 CHECK (ping_level IN ('essential', 'all', 'off'))
);

CREATE TABLE fixed_runs (
    id           TEXT PRIMARY KEY,
    owner_id     TEXT NOT NULL,
    channel_id   TEXT,
    bosses       TEXT NOT NULL,
    -- 0 = Monday, as v4 (Python weekday()).
    weekday      INTEGER NOT NULL CHECK (weekday BETWEEN 0 AND 6),
    -- Guild-local wall clock, HH:MM (HH:MM:SS only if seconds are set).
    time         TEXT NOT NULL,
    participants TEXT NOT NULL,
    note         TEXT
);

-- No FK to fixed_runs: retiring a weekly timing keeps its runs as history.
CREATE TABLE runs (
    id           TEXT PRIMARY KEY,
    fixed_run_id TEXT,
    channel_id   TEXT,
    week_start   TEXT NOT NULL,
    bosses       TEXT NOT NULL,
    datetime     TEXT NOT NULL,
    participants TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'planned'
                 CHECK (status IN ('planned', 'confirmed', 'at_risk', 'otot', 'done', 'cancelled')),
    source       TEXT NOT NULL DEFAULT 'fixed' CHECK (source IN ('fixed', 'amend'))
);
CREATE UNIQUE INDEX runs_fixed_week
    ON runs (fixed_run_id, week_start) WHERE fixed_run_id IS NOT NULL;
CREATE INDEX runs_by_week ON runs (week_start);

-- Deferred so one commit may delete and re-insert a run with its rows.
CREATE TABLE rsvps (
    run_id  TEXT NOT NULL REFERENCES runs (id) DEFERRABLE INITIALLY DEFERRED,
    user_id TEXT NOT NULL,
    state   TEXT NOT NULL CHECK (state IN ('yes', 'no', 'maybe')),
    source  TEXT NOT NULL DEFAULT 'reaction' CHECK (source IN ('reaction', 'chat', 'slash')),
    at      TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);

CREATE TABLE reminders (
    id         TEXT PRIMARY KEY,
    run_id     TEXT NOT NULL REFERENCES runs (id) DEFERRABLE INITIALLY DEFERRED,
    fire_at    TEXT NOT NULL,
    kind       TEXT NOT NULL,
    sent_at    TEXT,
    message_id TEXT,
    UNIQUE (run_id, kind)
);
CREATE INDEX reminders_pending ON reminders (sent_at, fire_at);
CREATE INDEX reminders_message ON reminders (message_id);

-- One live digest card per boss week; retired rows remain as the weekly log.
CREATE TABLE weekly_digests (
    week_start TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    posted_at  TEXT NOT NULL,
    retired_at TEXT
);
CREATE INDEX weekly_digests_message ON weekly_digests (message_id);

-- At most one decline notice per member and run.
CREATE TABLE decline_notices (
    run_id      TEXT NOT NULL,
    user_id     TEXT NOT NULL,
    channel_id  TEXT,
    message_id  TEXT,
    notified_at TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);

CREATE TABLE config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE audit (
    id      TEXT PRIMARY KEY,
    at      TEXT NOT NULL,
    surface TEXT NOT NULL DEFAULT 'system',
    actor   TEXT NOT NULL DEFAULT 'token',
    action  TEXT NOT NULL,
    subject TEXT,
    detail  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX audit_recent ON audit (at DESC);

-- Minimal maintenance authority; v4 upgrade-adoption and attestation columns
-- are not carried.
CREATE TABLE maintenance_state (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    mode           TEXT NOT NULL
                   CHECK (mode IN ('OPEN', 'PREPARING', 'BLOCKED', 'FROZEN', 'RESUMING')),
    generation     INTEGER NOT NULL DEFAULT 0 CHECK (generation >= 0),
    state_revision INTEGER NOT NULL DEFAULT 0 CHECK (state_revision >= 0),
    blocker_code   TEXT
);
INSERT INTO maintenance_state (id, mode) VALUES (1, 'OPEN');

CREATE TABLE maintenance_leases (
    operation_id      TEXT PRIMARY KEY,
    instance_id       TEXT NOT NULL,
    owner_token_hash  TEXT NOT NULL CHECK (
        length(owner_token_hash) = 64 AND owner_token_hash NOT GLOB '*[^0-9a-f]*'
    ),
    generation        INTEGER NOT NULL,
    operation_kind    TEXT NOT NULL,
    started_at        TEXT NOT NULL,
    lifecycle         TEXT NOT NULL DEFAULT 'live'
                      CHECK (lifecycle IN ('live', 'orphaned', 'retired')),
    orphaned_at       TEXT,
    retired_at        TEXT,
    retired_by        TEXT,
    retirement_reason TEXT,
    CHECK (lifecycle != 'orphaned' OR
           (orphaned_at IS NOT NULL AND length(trim(orphaned_at)) > 0)),
    CHECK ((lifecycle != 'retired') OR
           (retired_at IS NOT NULL AND length(trim(retired_at)) > 0 AND
            retired_by IS NOT NULL AND
            length(trim(retired_by)) BETWEEN 1 AND 128 AND
            length(trim(retirement_reason)) BETWEEN 1 AND 512)),
    UNIQUE (instance_id, owner_token_hash)
);
CREATE INDEX maintenance_leases_generation_started
    ON maintenance_leases (generation, started_at);

CREATE TABLE delivery_attempts (
    attempt_id             TEXT PRIMARY KEY,
    operation_id           TEXT NOT NULL,
    effect_ordinal         INTEGER NOT NULL,
    owner_instance_id      TEXT NOT NULL,
    origin                 TEXT NOT NULL CHECK (origin IN ('runtime', 'adoption', 'import')),
    effect_kind            TEXT NOT NULL,
    dedupe_scope           TEXT NOT NULL CHECK (dedupe_scope IN ('native', 'source', 'operation')),
    dedupe_key             TEXT NOT NULL CHECK (
        length(dedupe_key) = 64 AND dedupe_key NOT GLOB '*[^0-9a-f]*'
    ),
    dedupe_active          INTEGER NOT NULL DEFAULT 1 CHECK (dedupe_active IN (0, 1)),
    state                  TEXT NOT NULL
                           CHECK (state IN ('intent', 'indeterminate', 'bound', 'retired')),
    destination_kind       TEXT NOT NULL CHECK (destination_kind IN ('channel', 'dm')),
    guild_id               TEXT,
    channel_id             TEXT,
    recipient_id           TEXT,
    message_id             TEXT,
    fingerprint_version    INTEGER NOT NULL,
    request_fingerprint    TEXT NOT NULL,
    observable_fingerprint TEXT,
    intended_at            TEXT NOT NULL,
    resolved_at            TEXT,
    resolved_by            TEXT,
    resolution_reason      TEXT NOT NULL DEFAULT '',
    UNIQUE (operation_id, effect_ordinal),
    CHECK ((state != 'bound') OR (
        message_id IS NOT NULL AND length(trim(message_id)) > 0 AND
        ((destination_kind = 'channel' AND channel_id IS NOT NULL AND length(trim(channel_id)) > 0) OR
         (destination_kind = 'dm' AND recipient_id IS NOT NULL AND length(trim(recipient_id)) > 0))
    )),
    CHECK ((state = 'retired' AND dedupe_active = 0) OR (state != 'retired' AND dedupe_active = 1)),
    CHECK (
        length(request_fingerprint) = 64 AND request_fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (
        observable_fingerprint IS NULL OR
        (length(observable_fingerprint) = 64 AND observable_fingerprint NOT GLOB '*[^0-9a-f]*')
    ),
    CHECK (state != 'retired' OR (
        resolved_at IS NOT NULL AND length(trim(resolved_at)) > 0 AND
        resolved_by IS NOT NULL AND length(trim(resolved_by)) BETWEEN 1 AND 128 AND
        length(trim(resolution_reason)) BETWEEN 1 AND 512
    ))
);
CREATE UNIQUE INDEX delivery_attempts_active_dedupe
    ON delivery_attempts (dedupe_key) WHERE dedupe_active = 1;
CREATE INDEX delivery_attempts_state_intended ON delivery_attempts (state, intended_at);
CREATE INDEX delivery_attempts_message ON delivery_attempts (message_id);
CREATE INDEX delivery_attempts_operation ON delivery_attempts (operation_id);

-- No FK to native rows: a bound claim outlives the row it delivered.
CREATE TABLE delivery_attempt_targets (
    attempt_id     TEXT NOT NULL REFERENCES delivery_attempts (attempt_id),
    target_ordinal INTEGER NOT NULL,
    binding_type   TEXT NOT NULL
                   CHECK (binding_type IN ('reminder', 'digest', 'decline', 'card', 'debug_card')),
    key_primary    TEXT NOT NULL,
    key_secondary  TEXT,
    released_at    TEXT,
    release_actor  TEXT,
    release_reason TEXT,
    PRIMARY KEY (attempt_id, target_ordinal),
    CHECK ((released_at IS NULL AND release_actor IS NULL AND release_reason IS NULL) OR
           (released_at IS NOT NULL AND length(trim(released_at)) > 0 AND
            release_actor IS NOT NULL AND
            length(trim(release_actor)) BETWEEN 1 AND 128 AND
            length(trim(release_reason)) BETWEEN 1 AND 512))
);
CREATE UNIQUE INDEX delivery_attempt_targets_active_key
    ON delivery_attempt_targets (binding_type, key_primary, COALESCE(key_secondary, ''))
    WHERE released_at IS NULL;
