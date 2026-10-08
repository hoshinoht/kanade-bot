-- Member-session rotation (D9). `client_tag`: a keyed hash of the client
-- address a session was issued to, never the address itself; a request from
-- another address rotates the id. NULL: admin sessions, or a session from
-- before 0030. `superseded_until`: set only on an id rotated out, which stays
-- readable for safe requests until that instant and is pruned after it.
-- NULL: a live session.

ALTER TABLE web_sessions ADD COLUMN client_tag TEXT CHECK (
    client_tag IS NULL OR (length(client_tag) = 64 AND client_tag NOT GLOB '*[^0-9a-f]*')
);
ALTER TABLE web_sessions ADD COLUMN superseded_until TEXT;
