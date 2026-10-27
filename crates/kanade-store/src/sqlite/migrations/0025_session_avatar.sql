-- The Discord avatar hash a Discord sign-in reported (`/users/@me`), for the
-- Account portrait (`/api/admin/me/avatar`). NULL: no avatar, or a Tailscale
-- or token session. Existing sessions read back without one. A hash is 32
-- lowercase hex digits, or `a_` and 32 lowercase hex digits (animated).

ALTER TABLE web_sessions ADD COLUMN avatar_hash TEXT CHECK (
    avatar_hash IS NULL
    OR (length(avatar_hash) = 32 AND avatar_hash NOT GLOB '*[^0-9a-f]*')
    OR (
        length(avatar_hash) = 34
        AND substr(avatar_hash, 1, 2) = 'a_'
        AND substr(avatar_hash, 3) NOT GLOB '*[^0-9a-f]*'
    )
);
