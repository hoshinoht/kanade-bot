-- Pseudonymization (privacy/wire-ports).
--
-- 1. Extraction outcome `identity_leak`: a request the provider-boundary
--    scanner refused. SQLite cannot relax a CHECK in place, so `extractions`
--    and its child `extraction_members` are rebuilt under new names, the old
--    tables dropped (children first, so no foreign key is ever broken) and
--    the new ones renamed; indexes and the insert-only trigger are recreated
--    exactly as 0007/0008/0011 left them.
CREATE TABLE extractions_v14 (
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
        'self_service_link', 'identity_leak', 'unknown'
    )),
    error         TEXT,
    guardrail     TEXT NOT NULL CHECK (json_valid(guardrail) AND json_type(guardrail) = 'object'),
    message_ids   TEXT NOT NULL CHECK (json_valid(message_ids) AND json_type(message_ids) = 'array'),
    proposal_ids  TEXT NOT NULL CHECK (json_valid(proposal_ids) AND json_type(proposal_ids) = 'array'),
    refusals      TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(refusals) AND json_type(refusals) = 'array')
);
INSERT INTO extractions_v14 (id, at, channel_id, member_ids, model, reasoning, prompt,
    raw_response, latency_ms, request_count, outcome, error, guardrail, message_ids,
    proposal_ids, refusals)
SELECT id, at, channel_id, member_ids, model, reasoning, prompt, raw_response, latency_ms,
    request_count, outcome, error, guardrail, message_ids, proposal_ids, refusals
FROM extractions;

CREATE TABLE extraction_members_v14 (
    extraction_id TEXT NOT NULL REFERENCES extractions_v14 (id),
    member_id     TEXT NOT NULL,
    PRIMARY KEY (extraction_id, member_id)
);
INSERT INTO extraction_members_v14 (extraction_id, member_id)
SELECT extraction_id, member_id FROM extraction_members;

DROP TABLE extraction_members;
DROP TABLE extractions;
ALTER TABLE extractions_v14 RENAME TO extractions;
ALTER TABLE extraction_members_v14 RENAME TO extraction_members;

CREATE INDEX extractions_recent ON extractions (at, id);
CREATE INDEX extractions_model ON extractions (model, at);
CREATE INDEX extractions_outcome ON extractions (outcome, at);
CREATE INDEX extractions_channel ON extractions (channel_id, at);
CREATE TRIGGER extractions_no_update BEFORE UPDATE ON extractions
BEGIN SELECT RAISE(ABORT, 'extraction logs are insert-only'); END;

-- 2. The admin Model view of a masked chat turn (user decision D7): each
--    round's masked request and raw reply/tool-call arguments, the final
--    reply and the turn's token -> member mapping. Written with its
--    interaction, only while pseudonymization is on; insert-only; pruned
--    with the chat log.
CREATE TABLE chat_masked (
    interaction_id TEXT PRIMARY KEY REFERENCES chat_interactions (id),
    rounds         TEXT NOT NULL CHECK (json_valid(rounds) AND json_type(rounds) = 'array'),
    reply          TEXT NOT NULL,
    mapping        TEXT NOT NULL CHECK (json_valid(mapping) AND json_type(mapping) = 'array')
);
CREATE TRIGGER chat_masked_no_update BEFORE UPDATE ON chat_masked
BEGIN SELECT RAISE(ABORT, 'chat logs are insert-only'); END;
