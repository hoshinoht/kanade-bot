-- Sign-in audit log (user decision 2026-10-09): the security events of the
-- admin and member realms, kept 90 days. Append-only (UPDATE refused); every
-- append deletes the rows past retention in its own transaction. Member rows
-- carry only the keyed tag of the client address, admin rows the address.
-- The v4 `audit` table (0001) is unrelated and stays unused.
CREATE TABLE auth_audit (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    at         TEXT NOT NULL,
    realm      TEXT NOT NULL CHECK (realm IN ('admin', 'member')),
    event      TEXT NOT NULL CHECK (event IN (
        'login_succeeded', 'login_refused', 'break_glass_used', 'session_ended',
        'session_rotated', 'rate_limited', 'revoke_failed'
    )),
    actor      TEXT CHECK (actor IS NULL OR length(actor) BETWEEN 1 AND 256),
    method     TEXT CHECK (method IS NULL OR length(method) BETWEEN 1 AND 32),
    reason     TEXT CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 64),
    request    TEXT CHECK (request IS NULL OR length(request) BETWEEN 1 AND 256),
    client     TEXT CHECK (client IS NULL OR length(client) BETWEEN 1 AND 64),
    device     TEXT CHECK (device IS NULL OR length(device) BETWEEN 1 AND 64),
    request_id TEXT NOT NULL CHECK (length(request_id) <= 128)
);

CREATE INDEX auth_audit_at ON auth_audit (at);

CREATE TRIGGER auth_audit_append_only BEFORE UPDATE ON auth_audit
BEGIN
    SELECT RAISE(ABORT, 'auth_audit rows are append-only');
END;
