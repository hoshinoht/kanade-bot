-- Per-run character picks (module `roster`, user decisions 2026-10-12): the
-- character an admin picked for one party member on one run. Only explicit
-- picks are stored; a member's single active character is read as inferred
-- and never written. Outside the history chain (writes are audited in
-- `settings_changes`, section `characters`, key `pick.<run>.<member>`) and
-- never sent to a model. Deleting a character drops its picks (cascade).
-- No reference to runs: a scheduler commit rewrites run rows, and reads
-- ignore a pick whose member has left the party or whose run is gone.
CREATE TABLE roster_run_picks (
    run_id         TEXT NOT NULL CHECK (length(run_id) BETWEEN 1 AND 64),
    member_id      TEXT NOT NULL CHECK (length(member_id) BETWEEN 1 AND 32),
    character_id   TEXT NOT NULL REFERENCES roster_characters (id) ON DELETE CASCADE,
    picked_at      TEXT NOT NULL,
    picked_by_kind TEXT NOT NULL CHECK (picked_by_kind IN ('member', 'admin', 'system')),
    picked_by_id   TEXT NOT NULL CHECK (length(picked_by_id) BETWEEN 1 AND 320),
    version        INTEGER NOT NULL CHECK (version >= 1),
    PRIMARY KEY (run_id, member_id)
);

CREATE INDEX roster_run_picks_character ON roster_run_picks (character_id);
