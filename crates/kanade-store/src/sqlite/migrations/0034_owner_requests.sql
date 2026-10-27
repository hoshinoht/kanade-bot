-- Weekly-timing ownership requests (user decision 2026-10-10): a party member
-- asks to own a timing; its owner or staff accept or decline within 24 hours.
-- A row closes once (open -> accepted/declined/expired/withdrawn/superseded);
-- an accept pins the owner through a recorded schedule change.
CREATE TABLE owner_requests (
    id           TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 64),
    fixed_run_id TEXT NOT NULL,
    requester    TEXT NOT NULL CHECK (length(requester) BETWEEN 1 AND 32),
    channel_id   TEXT CHECK (channel_id IS NULL OR length(channel_id) BETWEEN 1 AND 32),
    message_id   TEXT CHECK (message_id IS NULL OR length(message_id) BETWEEN 1 AND 32),
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL CHECK (expires_at > created_at),
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN (
        'open', 'accepted', 'declined', 'expired', 'withdrawn', 'superseded'
    )),
    decided_by   TEXT CHECK (decided_by IS NULL OR length(decided_by) BETWEEN 1 AND 64),
    decided_at   TEXT,
    -- Its posted message was edited to show the decision (or is gone).
    message_settled INTEGER NOT NULL DEFAULT 0 CHECK (message_settled IN (0, 1)),
    CHECK ((status = 'open') = (decided_at IS NULL))
);

-- One open request per member and timing.
CREATE UNIQUE INDEX owner_requests_one_open
    ON owner_requests (fixed_run_id, requester) WHERE status = 'open';
CREATE INDEX owner_requests_due ON owner_requests (expires_at) WHERE status = 'open';
CREATE INDEX owner_requests_unsettled ON owner_requests (decided_at)
    WHERE status <> 'open' AND message_id IS NOT NULL AND message_settled = 0;

-- A decided request never reopens or changes.
CREATE TRIGGER owner_requests_close_once BEFORE UPDATE OF status ON owner_requests
WHEN OLD.status <> 'open'
BEGIN
    SELECT RAISE(ABORT, 'owner request already decided');
END;
