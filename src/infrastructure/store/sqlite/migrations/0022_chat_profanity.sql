-- Chat outcome `profanity` (the chat profanity guardrail; its side, matched
-- word and sent line live in the `guardrail` JSON object).
--
-- SQLite cannot relax a CHECK in place, and every connection enforces
-- foreign keys (`chat_rounds`, `chat_tools` and `chat_masked` reference
-- `chat_interactions`). The parent alone is rebuilt under its own name with
-- foreign-key checks deferred to the migration's commit: the rows are copied
-- aside, the table dropped and recreated, and the rows put back, so every
-- child row has its parent again before the commit checks. The children keep
-- their schema, rows and references untouched. Indexes and the insert-only
-- trigger are recreated exactly as 0007/0008 left them (0008 dropped
-- `chat_member` and `chat_latency`).
PRAGMA defer_foreign_keys = ON;

CREATE TABLE chat_interactions_v21 AS SELECT * FROM chat_interactions;

DROP TABLE chat_interactions;

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
        'turned_away', 'content_blocked', 'withheld', 'clean_retry', 'profanity',
        'unknown'
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
    completion_tokens INTEGER CHECK (completion_tokens >= 0),
    persona           TEXT,
    profile           TEXT,
    profile_source    TEXT
        CHECK (profile_source IS NULL OR profile_source IN ('saved', 'role', 'default')),
    error_code        TEXT
);

INSERT INTO chat_interactions (id, at, channel_id, message_id, member_id, question, reply,
    outcome, error, clean_retry, withheld, guardrail, request_count, latency_ms, model_ms,
    tools_ms, prompt_tokens, completion_tokens, persona, profile, profile_source, error_code)
SELECT id, at, channel_id, message_id, member_id, question, reply, outcome, error,
    clean_retry, withheld, guardrail, request_count, latency_ms, model_ms, tools_ms,
    prompt_tokens, completion_tokens, persona, profile, profile_source, error_code
FROM chat_interactions_v21;

DROP TABLE chat_interactions_v21;

CREATE INDEX chat_recent ON chat_interactions (at, id);
CREATE INDEX chat_outcome ON chat_interactions (outcome, at);
CREATE INDEX chat_channel ON chat_interactions (channel_id, at);
CREATE INDEX chat_flags ON chat_interactions (at) WHERE clean_retry = 1 OR withheld = 1;
CREATE TRIGGER chat_interactions_no_update BEFORE UPDATE ON chat_interactions
BEGIN SELECT RAISE(ABORT, 'chat logs are insert-only'); END;
