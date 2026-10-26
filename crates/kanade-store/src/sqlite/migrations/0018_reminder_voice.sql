-- Countdown phrases use reminder_cards.heading, while digest phrases must
-- exist before weekly_digests is created by bind.
CREATE TABLE reminder_cards_v18 (
    dedupe_key TEXT PRIMARY KEY CHECK (
        length(dedupe_key) = 64 AND dedupe_key NOT GLOB '*[^0-9a-f]*'
    ),
    kind       TEXT NOT NULL CHECK (kind = 'day_of' OR kind GLOB 'countdown_*'),
    heading    TEXT CHECK (heading IS NULL OR kind = 'day_of' OR kind GLOB 'countdown_*'),
    created_at TEXT NOT NULL
);
INSERT INTO reminder_cards_v18 (dedupe_key, kind, heading, created_at)
SELECT dedupe_key, kind, heading, created_at FROM reminder_cards;
DROP TABLE reminder_cards;
ALTER TABLE reminder_cards_v18 RENAME TO reminder_cards;
CREATE TRIGGER reminder_cards_no_update
BEFORE UPDATE ON reminder_cards
BEGIN
    SELECT RAISE(ABORT, 'reminder cards are written once');
END;

CREATE TABLE digest_card_phrases (
    dedupe_key TEXT PRIMARY KEY CHECK (
        length(dedupe_key) = 64 AND dedupe_key NOT GLOB '*[^0-9a-f]*'
    ),
    phrase     TEXT NOT NULL CHECK (length(trim(phrase)) BETWEEN 1 AND 48),
    created_at TEXT NOT NULL
);
CREATE TRIGGER digest_card_phrases_no_update
BEFORE UPDATE ON digest_card_phrases
BEGIN
    SELECT RAISE(ABORT, 'digest card phrases are written once');
END;
CREATE TRIGGER digest_card_phrases_no_delete
BEFORE DELETE ON digest_card_phrases
BEGIN
    SELECT RAISE(ABORT, 'digest card phrases are retained');
END;
