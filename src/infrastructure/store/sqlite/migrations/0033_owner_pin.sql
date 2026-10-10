-- Weekly-timing ownership (user decision 2026-10-09): a timing is owned by its
-- first participant unless staff pinned `owner_id`. Every existing timing
-- starts unpinned, so ownership moves to each party's first member.
ALTER TABLE fixed_runs ADD COLUMN owner_pinned INTEGER NOT NULL DEFAULT 0
    CHECK (owner_pinned IN (0, 1));
