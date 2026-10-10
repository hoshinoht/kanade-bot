-- The browser and system a sign-in came from ("Firefox · macOS"), derived
-- from its User-Agent when the session starts; the raw header is never
-- stored. Shown in the Account page's session list. NULL: not recognised,
-- or a session from before 0026.

ALTER TABLE web_sessions ADD COLUMN device TEXT CHECK (
    device IS NULL OR length(device) BETWEEN 1 AND 64
);
