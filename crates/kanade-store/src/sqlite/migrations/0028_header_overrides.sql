-- Manual header rewrites (`/debug rewrite`, `POST /api/admin/headers/rewrite`):
-- one insert-only row per accepted line, keyed by the native dedupe key of a
-- reminder card (`reminder_cards`) or a digest (`digest_card_phrases`). The
-- latest row of a key (highest `seq`) is shown instead of the original
-- heading or phrase, which stays as history. No retention.
CREATE TABLE header_overrides (
    seq        INTEGER PRIMARY KEY,
    dedupe_key TEXT NOT NULL CHECK (
        length(dedupe_key) = 64 AND dedupe_key NOT GLOB '*[^0-9a-f]*'
    ),
    line       TEXT NOT NULL CHECK (
        length(trim(line)) >= 1 AND length(CAST(line AS BLOB)) <= 1024
    ),
    actor      TEXT NOT NULL CHECK (length(CAST(actor AS BLOB)) BETWEEN 1 AND 128),
    created_at TEXT NOT NULL
);
CREATE INDEX header_overrides_key ON header_overrides (dedupe_key, seq);

CREATE TRIGGER header_overrides_no_update BEFORE UPDATE ON header_overrides
BEGIN SELECT RAISE(ABORT, 'header overrides are insert-only'); END;
CREATE TRIGGER header_overrides_no_delete BEFORE DELETE ON header_overrides
BEGIN SELECT RAISE(ABORT, 'header overrides are retained'); END;

-- The Rewrites log gains stage `manual`: rebuilt with the wider CHECK, rows
-- copied, its index and insert-only trigger recreated as 0027 left them.
CREATE TABLE rewrites_v28 (
    id                TEXT PRIMARY KEY,
    at                TEXT NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('day_of', 'countdown', 'digest', 'nudge')),
    stage             TEXT NOT NULL CHECK (stage IN (
        'batch', 'catchup', 'debug', 'nudge', 'manual'
    )),
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
INSERT INTO rewrites_v28 (
    id, at, kind, stage, context, verdict, rule, code, latency_ms, model, reasoning,
    prompt_tokens, completion_tokens, reasoning_tokens, reservation, budget,
    max_output_tokens, seed, reply, reasoning_content, line, request_id
)
SELECT
    id, at, kind, stage, context, verdict, rule, code, latency_ms, model, reasoning,
    prompt_tokens, completion_tokens, reasoning_tokens, reservation, budget,
    max_output_tokens, seed, reply, reasoning_content, line, request_id
FROM rewrites;
DROP TABLE rewrites;
ALTER TABLE rewrites_v28 RENAME TO rewrites;
CREATE INDEX rewrites_recent ON rewrites (at, id);

CREATE TRIGGER rewrites_no_update BEFORE UPDATE ON rewrites
BEGIN SELECT RAISE(ABORT, 'rewrite logs are insert-only'); END;
