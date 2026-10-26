-- Where the member gave their consent: Discord (/character link) or the
-- public portal. NULL while pending and on links consented before it was
-- recorded (read as unknown).
ALTER TABLE links ADD COLUMN consent_surface TEXT
    CHECK (consent_surface IS NULL
        OR (consent_surface IN ('discord', 'portal') AND status <> 'pending'));
