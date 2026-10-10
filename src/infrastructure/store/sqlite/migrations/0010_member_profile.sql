-- Member profile (domain::members::MemberStore): v4's aliases and reply
-- style, plus the gateway's role view the admin staff gate reads. Aliases are
-- unique across members through member_aliases, kept equal to the JSON list.

ALTER TABLE members ADD COLUMN aliases TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(aliases) AND json_type(aliases) = 'array');
ALTER TABLE members ADD COLUMN reply_style TEXT;
ALTER TABLE members ADD COLUMN roles TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(roles) AND json_type(roles) = 'array');
ALTER TABLE members ADD COLUMN is_guild_admin INTEGER NOT NULL DEFAULT 0
    CHECK (is_guild_admin IN (0, 1));

CREATE TABLE member_aliases (
    alias   TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES members (user_id) ON DELETE CASCADE
);
CREATE INDEX member_aliases_user ON member_aliases (user_id);
