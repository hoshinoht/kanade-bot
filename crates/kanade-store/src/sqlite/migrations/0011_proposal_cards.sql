-- Proposal cards (slice E5): what each proposal's Discord card shows, stored
-- with the proposal so it can be re-rendered after a restart, and where it
-- was posted. `message_id`/`posted_at` are written only by the delivery
-- journal's bind (binding type 'card', keyed by the proposal id).
CREATE TABLE proposal_cards (
    draft_id   TEXT PRIMARY KEY REFERENCES draft_proposals (draft_id),
    channel_id TEXT NOT NULL,
    details    TEXT NOT NULL CHECK (json_valid(details) AND json_type(details) = 'object'),
    message_id TEXT,
    posted_at  TEXT,
    created_at TEXT NOT NULL,
    CHECK ((message_id IS NULL) = (posted_at IS NULL))
);
CREATE INDEX proposal_cards_message ON proposal_cards (message_id)
    WHERE message_id IS NOT NULL;
CREATE INDEX proposal_cards_unposted ON proposal_cards (channel_id, created_at)
    WHERE message_id IS NULL;

-- Details never change; a card is bound once.
CREATE TRIGGER proposal_cards_fixed BEFORE UPDATE ON proposal_cards
WHEN NEW.draft_id IS NOT OLD.draft_id OR NEW.channel_id IS NOT OLD.channel_id
    OR NEW.details IS NOT OLD.details OR NEW.created_at IS NOT OLD.created_at
    OR OLD.message_id IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'a proposal card is written once and bound once'); END;
CREATE TRIGGER proposal_cards_no_delete BEFORE DELETE ON proposal_cards
BEGIN SELECT RAISE(ABORT, 'a proposal card is written once and bound once'); END;

-- Changes refused up front (D-PROPOSE-REFUSES), as [{change, code, message}].
ALTER TABLE extractions ADD COLUMN refusals TEXT NOT NULL DEFAULT '[]'
    CHECK (json_valid(refusals) AND json_type(refusals) = 'array');
