-- The Rewrites log (admin Rewrites page): one insert-only row per persona
-- rewrite attempt (the daily reminder-header batch and its catch-up, /debug trials,
-- self-service nudges) with its verdict, the gate rule or error code, the
-- route, usage against the runner's reservation (or the reservation against
-- the call budget when it was refused before sending) and the model's reply.
-- Pruned with the other model logs (`prune_model_logs`, 90 days).
CREATE TABLE rewrites (
    id                TEXT PRIMARY KEY,
    at                TEXT NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('day_of', 'countdown', 'digest', 'nudge')),
    stage             TEXT NOT NULL CHECK (stage IN ('batch', 'catchup', 'debug', 'nudge')),
    context           TEXT CHECK (context IS NULL OR length(CAST(context AS BLOB)) BETWEEN 1 AND 200),
    verdict           TEXT NOT NULL CHECK (verdict IN (
        'accepted', 'rejected', 'timeout', 'unavailable', 'refused', 'misconfigured',
        'no_rewriter', 'no_persona'
    )),
    rule              TEXT CHECK (rule IS NULL OR length(CAST(rule AS BLOB)) BETWEEN 1 AND 64),
    code              TEXT CHECK (code IS NULL OR length(CAST(code AS BLOB)) BETWEEN 1 AND 64),
    latency_ms        INTEGER CHECK (latency_ms IS NULL OR latency_ms >= 0),
    model             TEXT,
    reasoning         TEXT,
    prompt_tokens     INTEGER CHECK (prompt_tokens IS NULL OR prompt_tokens >= 0),
    completion_tokens INTEGER CHECK (completion_tokens IS NULL OR completion_tokens >= 0),
    reasoning_tokens  INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
    reservation       INTEGER CHECK (reservation IS NULL OR reservation >= 0),
    budget            INTEGER CHECK (budget IS NULL OR budget >= 0),
    max_output_tokens INTEGER CHECK (max_output_tokens IS NULL OR max_output_tokens >= 0),
    seed              TEXT NOT NULL CHECK (length(CAST(seed AS BLOB)) <= 1024),
    reply             TEXT CHECK (reply IS NULL OR length(CAST(reply AS BLOB)) <= 8192),
    reasoning_content TEXT CHECK (reasoning_content IS NULL OR
        length(CAST(reasoning_content AS BLOB)) BETWEEN 1 AND 65536),
    line              TEXT CHECK (line IS NULL OR length(CAST(line AS BLOB)) BETWEEN 1 AND 1024),
    request_id        TEXT,
    CHECK ((prompt_tokens IS NULL) = (completion_tokens IS NULL))
);
-- Lists walk it newest first; retention deletes oldest first.
CREATE INDEX rewrites_recent ON rewrites (at, id);

CREATE TRIGGER rewrites_no_update BEFORE UPDATE ON rewrites
BEGIN SELECT RAISE(ABORT, 'rewrite logs are insert-only'); END;
