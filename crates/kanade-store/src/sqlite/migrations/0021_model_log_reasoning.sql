-- Response-only diagnostics; existing rows remain NULL. Effort is unchanged.
ALTER TABLE extractions ADD COLUMN reasoning_content TEXT
    CHECK (reasoning_content IS NULL OR
        length(CAST(reasoning_content AS BLOB)) BETWEEN 1 AND 65536);
ALTER TABLE extractions ADD COLUMN reasoning_tokens INTEGER
    CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0);
ALTER TABLE chat_rounds ADD COLUMN reasoning_content TEXT
    CHECK (reasoning_content IS NULL OR
        length(CAST(reasoning_content AS BLOB)) BETWEEN 1 AND 65536);
ALTER TABLE chat_rounds ADD COLUMN reasoning_tokens INTEGER
    CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0);
