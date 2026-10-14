-- Saved Config sections, as History lists them (`domain::settings::audit`).
-- Append-only and outside the schedule's hash chain: rows are written with
-- the section's `config` rows in one transaction and never revert. `changes`
-- maps each changed settings key to `{"from", "to"}` row text; settings rows
-- never hold a secret.

CREATE TABLE settings_changes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         TEXT NOT NULL,
    actor_kind TEXT NOT NULL CHECK (actor_kind IN ('member', 'admin', 'system')),
    actor_id   TEXT NOT NULL CHECK (length(actor_id) BETWEEN 1 AND 320),
    surface    TEXT NOT NULL CHECK (length(surface) BETWEEN 1 AND 64),
    section    TEXT NOT NULL CHECK (length(section) BETWEEN 1 AND 64),
    revision   INTEGER NOT NULL CHECK (revision >= 0),
    changes    TEXT NOT NULL CHECK (
        json_valid(changes) AND json_type(changes) = 'object' AND changes <> '{}'
    )
);
CREATE INDEX settings_changes_at ON settings_changes (at, id);
CREATE TRIGGER settings_changes_no_update BEFORE UPDATE ON settings_changes
BEGIN
    SELECT RAISE(ABORT, 'settings history is append-only');
END;
CREATE TRIGGER settings_changes_no_delete BEFORE DELETE ON settings_changes
BEGIN
    SELECT RAISE(ABORT, 'settings history is append-only');
END;
