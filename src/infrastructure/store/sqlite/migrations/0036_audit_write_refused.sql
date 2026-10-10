-- The sign-in audit log also keeps refused member writes (`write_refused`:
-- the route in `request`, the refusal code in `reason`). `auth_audit` is
-- rebuilt with the wider event CHECK, every row copied with its `seq`, and
-- its index and append-only trigger recreated as 0032 left them. Nothing
-- references the table, so no foreign key is involved.
CREATE TABLE auth_audit_v36 (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    at         TEXT NOT NULL,
    realm      TEXT NOT NULL CHECK (realm IN ('admin', 'member')),
    event      TEXT NOT NULL CHECK (event IN (
        'login_succeeded', 'login_refused', 'break_glass_used', 'session_ended',
        'session_rotated', 'rate_limited', 'revoke_failed', 'write_refused'
    )),
    actor      TEXT CHECK (actor IS NULL OR length(actor) BETWEEN 1 AND 256),
    method     TEXT CHECK (method IS NULL OR length(method) BETWEEN 1 AND 32),
    reason     TEXT CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 64),
    request    TEXT CHECK (request IS NULL OR length(request) BETWEEN 1 AND 256),
    client     TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 64),
    device     TEXT CHECK (device IS NULL OR length(device) BETWEEN 1 AND 64),
    request_id TEXT NOT NULL CHECK (length(request_id) <= 128)
);
INSERT INTO auth_audit_v36 (
    seq, at, realm, event, actor, method, reason, request, client, device, request_id
)
SELECT seq, at, realm, event, actor, method, reason, request, client, device, request_id
FROM auth_audit;
-- Keep the AUTOINCREMENT high-water mark, so a pruned seq is never reused.
DELETE FROM sqlite_sequence WHERE name = 'auth_audit_v36';
INSERT INTO sqlite_sequence (name, seq)
SELECT 'auth_audit_v36', seq FROM sqlite_sequence WHERE name = 'auth_audit';
DROP TABLE auth_audit;
ALTER TABLE auth_audit_v36 RENAME TO auth_audit;

CREATE INDEX auth_audit_at ON auth_audit (at);

CREATE TRIGGER auth_audit_append_only BEFORE UPDATE ON auth_audit
BEGIN
    SELECT RAISE(ABORT, 'auth_audit rows are append-only');
END;
