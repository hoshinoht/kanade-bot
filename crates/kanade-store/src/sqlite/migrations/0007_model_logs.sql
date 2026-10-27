-- Extraction and chat persistence (domain::model_log) and the proposal draft
-- kind (domain::drafts::ProposalStore). Instants are UTC ISO text; JSON
-- columns are checked for shape. Logs are insert-only; the filter side
-- tables (`extraction_members`, `chat_tools`) are derived on insert.

-- Watched-message cache (v4 `messages` plus `edited_at`).
CREATE TABLE messages (
    id           TEXT PRIMARY KEY,
    channel_id   TEXT NOT NULL,
    author_id    TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    edited_at    TEXT,
    content      TEXT NOT NULL,
    processed_at TEXT
);
CREATE INDEX messages_channel ON messages (channel_id, created_at, id);
CREATE INDEX messages_unprocessed ON messages (channel_id, created_at, id)
    WHERE processed_at IS NULL;

CREATE TABLE extractions (
    id            TEXT PRIMARY KEY,
    at            TEXT NOT NULL,
    channel_id    TEXT,
    member_ids    TEXT NOT NULL CHECK (json_valid(member_ids) AND json_type(member_ids) = 'array'),
    model         TEXT NOT NULL,
    reasoning     TEXT,
    prompt        TEXT NOT NULL,
    raw_response  TEXT NOT NULL,
    latency_ms    INTEGER CHECK (latency_ms >= 0),
    request_count INTEGER NOT NULL CHECK (request_count >= 0),
    outcome       TEXT NOT NULL CHECK (outcome IN (
        'proposed', 'no_change', 'failed', 'turned_away', 'content_blocked',
        'self_service_link', 'unknown'
    )),
    error         TEXT,
    guardrail     TEXT NOT NULL CHECK (json_valid(guardrail) AND json_type(guardrail) = 'object'),
    message_ids   TEXT NOT NULL CHECK (json_valid(message_ids) AND json_type(message_ids) = 'array'),
    proposal_ids  TEXT NOT NULL CHECK (json_valid(proposal_ids) AND json_type(proposal_ids) = 'array')
);
CREATE INDEX extractions_recent ON extractions (at, id);
CREATE INDEX extractions_model ON extractions (model, at);
CREATE INDEX extractions_outcome ON extractions (outcome, at);
CREATE INDEX extractions_channel ON extractions (channel_id, at);

CREATE TABLE extraction_members (
    extraction_id TEXT NOT NULL REFERENCES extractions (id),
    member_id     TEXT NOT NULL,
    PRIMARY KEY (extraction_id, member_id)
);
CREATE INDEX extraction_members_member ON extraction_members (member_id, extraction_id);

CREATE TABLE chat_interactions (
    id                TEXT PRIMARY KEY,
    at                TEXT NOT NULL,
    channel_id        TEXT,
    message_id        TEXT,
    member_id         TEXT,
    question          TEXT NOT NULL,
    reply             TEXT NOT NULL,
    outcome           TEXT NOT NULL CHECK (outcome IN (
        'answered', 'refused', 'clarified', 'error', 'timeout', 'rate_limited',
        'turned_away', 'content_blocked', 'withheld', 'clean_retry', 'unknown'
    )),
    error             TEXT,
    clean_retry       INTEGER NOT NULL CHECK (clean_retry IN (0, 1)),
    withheld          INTEGER NOT NULL CHECK (withheld IN (0, 1)),
    guardrail         TEXT NOT NULL CHECK (json_valid(guardrail) AND json_type(guardrail) = 'object'),
    request_count     INTEGER NOT NULL CHECK (request_count >= 0),
    latency_ms        INTEGER CHECK (latency_ms >= 0),
    model_ms          INTEGER CHECK (model_ms >= 0),
    tools_ms          INTEGER CHECK (tools_ms >= 0),
    prompt_tokens     INTEGER CHECK (prompt_tokens >= 0),
    completion_tokens INTEGER CHECK (completion_tokens >= 0)
);
CREATE INDEX chat_recent ON chat_interactions (at, id);
CREATE INDEX chat_outcome ON chat_interactions (outcome, at);
CREATE INDEX chat_channel ON chat_interactions (channel_id, at);
CREATE INDEX chat_member ON chat_interactions (member_id, at);
CREATE INDEX chat_latency ON chat_interactions (latency_ms);
CREATE INDEX chat_flags ON chat_interactions (at) WHERE clean_retry = 1 OR withheld = 1;

CREATE TABLE chat_rounds (
    interaction_id TEXT NOT NULL REFERENCES chat_interactions (id),
    ord            INTEGER NOT NULL CHECK (ord >= 0),
    model          TEXT NOT NULL,
    reasoning      TEXT,
    finish_reason  TEXT,
    latency_ms     INTEGER CHECK (latency_ms >= 0),
    tool_bundles   TEXT NOT NULL CHECK (json_valid(tool_bundles) AND json_type(tool_bundles) = 'array'),
    tools          TEXT NOT NULL CHECK (json_valid(tools) AND json_type(tools) = 'array'),
    tool_calls     TEXT NOT NULL CHECK (json_valid(tool_calls) AND json_type(tool_calls) = 'array'),
    response       TEXT,
    PRIMARY KEY (interaction_id, ord)
);
CREATE INDEX chat_rounds_model ON chat_rounds (model, interaction_id);

-- Distinct tools an interaction called (the `tool` filter).
CREATE TABLE chat_tools (
    interaction_id TEXT NOT NULL REFERENCES chat_interactions (id),
    tool           TEXT NOT NULL,
    PRIMARY KEY (interaction_id, tool)
);
CREATE INDEX chat_tools_tool ON chat_tools (tool, interaction_id);

CREATE TRIGGER extractions_no_update BEFORE UPDATE ON extractions
BEGIN SELECT RAISE(ABORT, 'extraction logs are insert-only'); END;
CREATE TRIGGER chat_interactions_no_update BEFORE UPDATE ON chat_interactions
BEGIN SELECT RAISE(ABORT, 'chat logs are insert-only'); END;
CREATE TRIGGER chat_rounds_no_update BEFORE UPDATE ON chat_rounds
BEGIN SELECT RAISE(ABORT, 'chat logs are insert-only'); END;

-- v4 `rescan_jobs`; `window` is kept as given (v4 and v5 spell it differently).
CREATE TABLE rescan_jobs (
    id           TEXT PRIMARY KEY,
    channels     TEXT NOT NULL CHECK (json_valid(channels) AND json_type(channels) = 'array'),
    window       TEXT NOT NULL CHECK (length(window) >= 1),
    source       TEXT NOT NULL,
    automated    INTEGER NOT NULL CHECK (automated IN (0, 1)),
    requested_by TEXT,
    status       TEXT NOT NULL CHECK (status IN ('queued', 'running', 'done', 'failed', 'cancelled')),
    created_at   TEXT NOT NULL,
    started_at   TEXT,
    finished_at  TEXT,
    results      TEXT NOT NULL CHECK (json_valid(results) AND json_type(results) = 'array'),
    error        TEXT
);
CREATE INDEX rescan_jobs_recent ON rescan_jobs (created_at, id);

-- v4 `chat_rate_limits`; the window is whole milliseconds.
CREATE TABLE chat_allowance_overrides (
    member_id  TEXT PRIMARY KEY,
    count      INTEGER NOT NULL CHECK (count >= 0),
    window_ms  INTEGER NOT NULL CHECK (window_ms > 0),
    updated_at TEXT NOT NULL
);

-- At most one self-service tip per member and boss week.
CREATE TABLE self_service_tips (
    member_id TEXT NOT NULL,
    week      TEXT NOT NULL,
    sent_at   TEXT NOT NULL,
    PRIMARY KEY (member_id, week)
);

-- Proposal drafts. `drafts.kind` stays 'admin' (its 0005 CHECK is not
-- rebuilt); a row here makes the draft a proposal. Written with the draft
-- and never changed.
CREATE TABLE draft_proposals (
    draft_id      TEXT PRIMARY KEY REFERENCES drafts (id),
    source        TEXT NOT NULL CHECK (source IN ('extraction', 'chat')),
    source_id     TEXT NOT NULL,
    supersede_key TEXT,
    expires_at    TEXT NOT NULL
);
CREATE INDEX draft_proposals_key ON draft_proposals (supersede_key)
    WHERE supersede_key IS NOT NULL;
CREATE INDEX draft_proposals_expiry ON draft_proposals (expires_at);

CREATE TRIGGER draft_proposals_admin_system BEFORE INSERT ON draft_proposals
WHEN NOT EXISTS (
    SELECT 1 FROM drafts
    WHERE id = NEW.draft_id AND kind = 'admin' AND author_kind = 'system'
)
BEGIN SELECT RAISE(ABORT, 'a proposal is a system-authored admin-kind draft'); END;
CREATE TRIGGER draft_proposals_no_update BEFORE UPDATE ON draft_proposals
BEGIN SELECT RAISE(ABORT, 'proposal facts never change'); END;
CREATE TRIGGER draft_proposals_no_delete BEFORE DELETE ON draft_proposals
BEGIN SELECT RAISE(ABORT, 'proposal facts never change'); END;
