-- Chat turn facts for the admin transcript (surfaces/transcript-fields):
-- the persona and reply profile a turn answered with, a stable error code,
-- and per round the route and whether it was the clean retry. The round's
-- `model` and `reasoning` hold what the request actually sent. Nullable or
-- defaulted, so earlier rows stay valid; logs stay insert-only.
ALTER TABLE chat_interactions ADD COLUMN persona TEXT;
ALTER TABLE chat_interactions ADD COLUMN profile TEXT;
ALTER TABLE chat_interactions ADD COLUMN profile_source TEXT
    CHECK (profile_source IS NULL OR profile_source IN ('saved', 'role', 'default'));
ALTER TABLE chat_interactions ADD COLUMN error_code TEXT;
ALTER TABLE chat_rounds ADD COLUMN route TEXT
    CHECK (route IS NULL OR route IN ('homelab', 'external_masked', 'external_unmasked'));
ALTER TABLE chat_rounds ADD COLUMN clean INTEGER NOT NULL DEFAULT 0 CHECK (clean IN (0, 1));
