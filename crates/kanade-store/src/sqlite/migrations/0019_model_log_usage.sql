-- 0019_model_log_usage.sql
-- Provider-reported token usage per extraction call (summed over attempts
-- that reported a pair) and per chat round, plus the local prompt estimate.
-- NULL = not reported (older rows, v4 imports, failed/cut calls). A pair is
-- all-or-nothing. Logs stay insert-only (existing triggers cover new columns).
ALTER TABLE extractions ADD COLUMN prompt_tokens INTEGER
    CHECK (prompt_tokens >= 0);
ALTER TABLE extractions ADD COLUMN completion_tokens INTEGER
    CHECK (completion_tokens >= 0)
    CHECK ((prompt_tokens IS NULL) = (completion_tokens IS NULL));
ALTER TABLE extractions ADD COLUMN prompt_estimate INTEGER
    CHECK (prompt_estimate >= 0);
ALTER TABLE chat_rounds ADD COLUMN prompt_tokens INTEGER
    CHECK (prompt_tokens >= 0);
ALTER TABLE chat_rounds ADD COLUMN completion_tokens INTEGER
    CHECK (completion_tokens >= 0)
    CHECK ((prompt_tokens IS NULL) = (completion_tokens IS NULL));
ALTER TABLE chat_rounds ADD COLUMN prompt_estimate INTEGER
    CHECK (prompt_estimate >= 0);
