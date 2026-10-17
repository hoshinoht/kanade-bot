-- "OK to be teased": the member's own opt-in (off for everyone until they
-- or an admin turn it on) and an admin's short running-joke note, kept
-- after an opt-out but only ever sent while the member is opted in.
ALTER TABLE members ADD COLUMN tease_ok INTEGER NOT NULL DEFAULT 0 CHECK (tease_ok IN (0, 1));
ALTER TABLE members ADD COLUMN running_joke TEXT CHECK (
    running_joke IS NULL OR length(running_joke) BETWEEN 1 AND 80
);
