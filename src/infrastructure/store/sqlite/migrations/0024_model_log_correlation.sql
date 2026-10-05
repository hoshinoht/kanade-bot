-- Gateway correlation: the governed session stem and the exact `x-request-id`
-- values sent, so an operator can find a log row's requests in the gateway's
-- own request log. Additive and nullable: existing rows stay NULL (not
-- recorded). Ids are never secrets; keys and headers other than the id are
-- never stored.
ALTER TABLE chat_interactions ADD COLUMN session_id TEXT
    CHECK (session_id IS NULL OR length(session_id) BETWEEN 1 AND 128);
ALTER TABLE chat_rounds ADD COLUMN request_ids TEXT
    CHECK (request_ids IS NULL OR (json_valid(request_ids)
        AND json_type(request_ids) = 'array' AND json_array_length(request_ids) > 0));
ALTER TABLE extractions ADD COLUMN session_id TEXT
    CHECK (session_id IS NULL OR length(session_id) BETWEEN 1 AND 128);
ALTER TABLE extractions ADD COLUMN request_ids TEXT
    CHECK (request_ids IS NULL OR (json_valid(request_ids)
        AND json_type(request_ids) = 'array' AND json_array_length(request_ids) > 0));
