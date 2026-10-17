-- Reminder header lines written with estimate context keep `{cN}`
-- placeholders; the slot map beside them (JSON: the slots with each
-- character id, member id, IGN and tease flag, plus `used_estimates`) is
-- what every render fills them from after its send-time checks. NULL for
-- every line written without context, so existing rows read as before.
ALTER TABLE reminder_cards ADD COLUMN slots TEXT CHECK (
    slots IS NULL OR (json_valid(slots) AND length(CAST(slots AS BLOB)) <= 4096)
);
ALTER TABLE header_overrides ADD COLUMN slots TEXT CHECK (
    slots IS NULL OR (json_valid(slots) AND length(CAST(slots AS BLOB)) <= 4096)
);
