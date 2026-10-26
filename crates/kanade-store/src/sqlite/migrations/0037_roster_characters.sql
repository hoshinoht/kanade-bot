-- Manual character roster (module `roster`, user decisions 2026-10-11): the
-- MapleStory characters an admin records for a member. Outside the history
-- chain (writes are audited in `settings_changes`, section `characters`),
-- never sent to a model, kept when the member leaves the guild and removed
-- only by an explicit admin delete. No reference to runs or members.
-- The IGN rule itself is enforced on input, not here, so it can change
-- without rebuilding the table.
CREATE TABLE roster_characters (
    id              TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 64),
    member_id       TEXT NOT NULL CHECK (length(member_id) BETWEEN 1 AND 32),
    ign             TEXT NOT NULL CHECK (length(ign) BETWEEN 1 AND 32),
    -- Case-insensitive, unique per world (region), retired characters included.
    ign_key         TEXT NOT NULL CHECK (ign_key = lower(ign)),
    world           TEXT NOT NULL DEFAULT 'msea' CHECK (length(world) BETWEEN 1 AND 16),
    class           TEXT CHECK (class IS NULL OR length(class) BETWEEN 1 AND 32),
    level           INTEGER CHECK (level IS NULL OR level BETWEEN 1 AND 300),
    role            TEXT NOT NULL DEFAULT 'alt' CHECK (role IN ('main', 'alt')),
    retired_at      TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    updated_by_kind TEXT NOT NULL CHECK (updated_by_kind IN ('member', 'admin', 'system')),
    updated_by_id   TEXT NOT NULL CHECK (length(updated_by_id) BETWEEN 1 AND 320),
    version         INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE (world, ign_key)
);

CREATE INDEX roster_characters_member ON roster_characters (member_id, created_at, id);
-- At most one active main per member.
CREATE UNIQUE INDEX roster_characters_one_main ON roster_characters (member_id)
    WHERE role = 'main' AND retired_at IS NULL;
