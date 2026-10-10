-- The prompt a rewrite sent: its system and user messages under role labels
-- (persona and seed text only), shown in the admin Rewrite detail and its
-- copied transcript. NULL: nothing was sent, or a row from before 0029.
-- Pruned with the row.
ALTER TABLE rewrites ADD COLUMN prompt TEXT CHECK (
    prompt IS NULL OR length(CAST(prompt AS BLOB)) BETWEEN 1 AND 16384
);
