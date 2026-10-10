-- Notice outbox: every notice a decision asks for, written in the decision's
-- own transaction (a change record's commit or merge, or a draft close), so a
-- crash can neither lose it nor, on a retry answered AlreadyApplied, repeat
-- it. `source` names the decision (`change:<seq>` or `draft:<id>`) and
-- `ordinal` the notice's position in it. `payload` is the domain notice (its
-- intent, versioned JSON); channel choice, mentions and text are derived when
-- the delivery tick drains it through the journal (source-scoped dedupe).
CREATE TABLE notice_outbox (
    id          INTEGER PRIMARY KEY,
    source      TEXT NOT NULL CHECK (length(source) BETWEEN 1 AND 128),
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 0),
    effect_kind TEXT NOT NULL CHECK (length(effect_kind) BETWEEN 1 AND 128),
    payload     TEXT NOT NULL CHECK (json_valid(payload) AND json_type(payload) = 'object'),
    created_at  TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'drained')),
    drained_at  TEXT,
    CHECK ((state = 'pending') = (drained_at IS NULL)),
    UNIQUE (source, ordinal)
);
CREATE INDEX notice_outbox_pending ON notice_outbox (id) WHERE state = 'pending';

-- Only pending -> drained, once; nothing else ever changes.
CREATE TRIGGER notice_outbox_final BEFORE UPDATE ON notice_outbox
WHEN OLD.state = 'drained' OR NEW.state IS NOT 'drained'
    OR NEW.id IS NOT OLD.id OR NEW.source IS NOT OLD.source
    OR NEW.ordinal IS NOT OLD.ordinal OR NEW.effect_kind IS NOT OLD.effect_kind
    OR NEW.payload IS NOT OLD.payload OR NEW.created_at IS NOT OLD.created_at
BEGIN SELECT RAISE(ABORT, 'an outbox notice is only ever drained once'); END;

-- Source-scoped claims look up retired attempts by key too.
CREATE INDEX delivery_attempts_dedupe_key ON delivery_attempts (dedupe_key);
