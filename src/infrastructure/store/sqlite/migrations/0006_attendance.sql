-- v5 attendance (domain::attendance, docs/v5/attendance.md): a weekly
-- timing's default for unanswered members, members' standing answers
-- ("always in") per timing, and attendance recorded per run.

ALTER TABLE fixed_runs ADD COLUMN attendance_default TEXT NOT NULL DEFAULT 'opt_in'
    CHECK (attendance_default IN ('opt_in', 'assume_coming'));

-- Written with the timing's row (the domain keeps them on `FixedRun`), so a
-- change record's before/after of the timing includes them.
CREATE TABLE standing_answers (
    fixed_run_id TEXT NOT NULL REFERENCES fixed_runs (id) DEFERRABLE INITIALLY DEFERRED,
    user_id      TEXT NOT NULL,
    set_by       TEXT NOT NULL,
    at           TEXT NOT NULL,
    PRIMARY KEY (fixed_run_id, user_id)
);
CREATE INDEX standing_answers_user ON standing_answers (user_id);

CREATE TABLE run_attendance (
    run_id      TEXT NOT NULL REFERENCES runs (id) DEFERRABLE INITIALLY DEFERRED,
    user_id     TEXT NOT NULL,
    attended    INTEGER NOT NULL CHECK (attended IN (0, 1)),
    recorded_by TEXT NOT NULL,
    at          TEXT NOT NULL,
    PRIMARY KEY (run_id, user_id)
);
CREATE INDEX run_attendance_user ON run_attendance (user_id, run_id);

-- A status an administrator set by hand, kept by v5 derivation; written
-- with the run's row (never in v4-compat mode).
CREATE TABLE run_status_pins (
    run_id TEXT PRIMARY KEY REFERENCES runs (id) DEFERRABLE INITIALLY DEFERRED,
    status TEXT NOT NULL CHECK (status IN ('planned', 'confirmed')),
    at     TEXT NOT NULL
);
