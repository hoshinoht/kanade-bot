-- Per-boss character picks (module `roster`, user decisions 2026-10-16): on
-- a weekly timing or run with several bosses, the character an admin picked
-- for one party member for one boss after the first. `boss` is the catalog
-- boss (`MaleficStar`, any difficulty), so a pick survives a difficulty
-- change. The first boss's character is the pick in `roster_run_picks` /
-- `roster_fixed_picks` (else the main, else the only active character), and
-- every other boss without a pick of its own brings that character. Only
-- explicit picks are stored; reads ignore a pick whose boss is no longer on
-- the run or timing, or is now its first boss, and keep the row, so undoing
-- the change brings the pick back. Outside the history chain (writes are
-- audited in `settings_changes`, section `characters`, key
-- `pick.<run>.<member>.<boss>` or `fixedpick.<fixed>.<member>.<boss>`) and
-- never sent to a model. Deleting a character drops its picks (cascade).
CREATE TABLE roster_run_boss_picks (
    run_id         TEXT NOT NULL CHECK (length(run_id) BETWEEN 1 AND 64),
    member_id      TEXT NOT NULL CHECK (length(member_id) BETWEEN 1 AND 32),
    boss           TEXT NOT NULL CHECK (length(boss) BETWEEN 1 AND 64),
    character_id   TEXT NOT NULL REFERENCES roster_characters (id) ON DELETE CASCADE,
    picked_at      TEXT NOT NULL,
    picked_by_kind TEXT NOT NULL CHECK (picked_by_kind IN ('member', 'admin', 'system')),
    picked_by_id   TEXT NOT NULL CHECK (length(picked_by_id) BETWEEN 1 AND 320),
    version        INTEGER NOT NULL CHECK (version >= 1),
    PRIMARY KEY (run_id, member_id, boss)
);

CREATE INDEX roster_run_boss_picks_character ON roster_run_boss_picks (character_id);

CREATE TABLE roster_fixed_boss_picks (
    fixed_run_id   TEXT NOT NULL CHECK (length(fixed_run_id) BETWEEN 1 AND 64),
    member_id      TEXT NOT NULL CHECK (length(member_id) BETWEEN 1 AND 32),
    boss           TEXT NOT NULL CHECK (length(boss) BETWEEN 1 AND 64),
    character_id   TEXT NOT NULL REFERENCES roster_characters (id) ON DELETE CASCADE,
    picked_at      TEXT NOT NULL,
    picked_by_kind TEXT NOT NULL CHECK (picked_by_kind IN ('member', 'admin', 'system')),
    picked_by_id   TEXT NOT NULL CHECK (length(picked_by_id) BETWEEN 1 AND 320),
    version        INTEGER NOT NULL CHECK (version >= 1),
    PRIMARY KEY (fixed_run_id, member_id, boss)
);

CREATE INDEX roster_fixed_boss_picks_character ON roster_fixed_boss_picks (character_id);
