-- Log retention and index cleanup (EXPLAIN QUERY PLAN pins in
-- sqlite/model_log/plans.rs). List filters are `?n IS NULL OR …`, so the
-- lists walk the (at, id) index newest first and stop at the page limit;
-- these 0007 indexes serve no statement.
DROP INDEX chat_member;
DROP INDEX chat_latency;
DROP INDEX extraction_members_member;

-- Retention deletes processed cache rows oldest first.
CREATE INDEX messages_processed ON messages (created_at, id)
    WHERE processed_at IS NOT NULL;
