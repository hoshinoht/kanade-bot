-- Provider-reported cached prompt tokens (`usage.prompt_tokens_details.cached_tokens`):
-- the part of `prompt_tokens` the upstream served from its prompt cache. NULL = not
-- reported (older rows, providers without prompt caching, failed calls); never estimated.
-- Interaction totals sum the rounds that reported it. Logs stay insert-only.
ALTER TABLE extractions ADD COLUMN cached_tokens INTEGER
    CHECK (cached_tokens IS NULL OR cached_tokens >= 0);
ALTER TABLE chat_interactions ADD COLUMN cached_tokens INTEGER
    CHECK (cached_tokens IS NULL OR cached_tokens >= 0);
ALTER TABLE chat_rounds ADD COLUMN cached_tokens INTEGER
    CHECK (cached_tokens IS NULL OR cached_tokens >= 0);
ALTER TABLE rewrites ADD COLUMN cached_tokens INTEGER
    CHECK (cached_tokens IS NULL OR cached_tokens >= 0);
