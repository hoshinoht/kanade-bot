-- Scouter estimates (module character_data): each character's latest result
-- and each party's latest total, a cache kept at most 30 days. Never in the
-- change history or backups. `result` is kanade's stored JSON object.

-- Keyed by roster character; `ign_key` is the IGN the result was read for,
-- so a renamed character's result stops matching.
CREATE TABLE estimate_members (
    character_id TEXT PRIMARY KEY CHECK (length(character_id) > 0),
    ign_key      TEXT NOT NULL CHECK (length(ign_key) > 0),
    result       TEXT NOT NULL CHECK (json_valid(result)),
    fetched_at   TEXT NOT NULL
);
CREATE INDEX estimate_members_fetched ON estimate_members (fetched_at);

-- `set_key`: the sorted, lowercase IGN set the total was asked for.
CREATE TABLE estimate_parties (
    set_key    TEXT PRIMARY KEY CHECK (length(set_key) > 0),
    result     TEXT NOT NULL CHECK (json_valid(result)),
    fetched_at TEXT NOT NULL
);
CREATE INDEX estimate_parties_fetched ON estimate_parties (fetched_at);

-- The characters a party total includes: deleting one character's
-- estimates deletes every total naming them, and these rows with it.
CREATE TABLE estimate_party_members (
    set_key      TEXT NOT NULL REFERENCES estimate_parties (set_key) ON DELETE CASCADE,
    character_id TEXT NOT NULL CHECK (length(character_id) > 0),
    PRIMARY KEY (set_key, character_id)
) WITHOUT ROWID;
CREATE INDEX estimate_party_members_character ON estimate_party_members (character_id);
