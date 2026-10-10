-- Which runs a delivered card is for, written with the binding. Kept out of
-- the dedupe identity; rows outlive the reminder rows they were derived from,
-- so reactions still map after a rebuild deletes them.
CREATE TABLE delivery_card_runs (
    attempt_id TEXT NOT NULL REFERENCES delivery_attempts (attempt_id),
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    run_id     TEXT NOT NULL,
    PRIMARY KEY (attempt_id, run_id)
);
CREATE INDEX delivery_card_runs_card ON delivery_card_runs (channel_id, message_id);
-- CardIndex receives only the message id (a Discord snowflake, unique).
CREATE INDEX delivery_card_runs_message ON delivery_card_runs (message_id);
