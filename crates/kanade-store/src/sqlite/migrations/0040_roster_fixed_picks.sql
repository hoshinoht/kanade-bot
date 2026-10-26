-- Per-timing character picks (modules `roster` + `character_data`, user
-- decisions 2026-10-13): the character an admin picked for one party member
-- of one weekly timing. Only explicit picks are stored; the main / only
-- character defaults are computed on read, and runs made from the timing
-- read the pick live (a run's own pick in `roster_run_picks` still wins).
-- Outside the history chain (writes are audited in `settings_changes`,
-- section `characters`, key `fixedpick.<fixed>.<member>`) and never sent to
-- a model. Deleting a character drops its picks (cascade). No reference to
-- `fixed_runs`: retiring a timing deletes its row, and reads ignore a pick
-- whose timing is gone or whose member has left its party, so undoing the
-- retire brings the picks back.
CREATE TABLE roster_fixed_picks (
    fixed_run_id   TEXT NOT NULL CHECK (length(fixed_run_id) BETWEEN 1 AND 64),
    member_id      TEXT NOT NULL CHECK (length(member_id) BETWEEN 1 AND 32),
    character_id   TEXT NOT NULL REFERENCES roster_characters (id) ON DELETE CASCADE,
    picked_at      TEXT NOT NULL,
    picked_by_kind TEXT NOT NULL CHECK (picked_by_kind IN ('member', 'admin', 'system')),
    picked_by_id   TEXT NOT NULL CHECK (length(picked_by_id) BETWEEN 1 AND 320),
    version        INTEGER NOT NULL CHECK (version >= 1),
    PRIMARY KEY (fixed_run_id, member_id)
);

CREATE INDEX roster_fixed_picks_character ON roster_fixed_picks (character_id);
