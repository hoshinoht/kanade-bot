-- Start-up recovery orphans the live delivery-journal leases: find them
-- without scanning every ended one. IF NOT EXISTS: tests that wind the
-- ledger back to an older version keep the index.
CREATE INDEX IF NOT EXISTS maintenance_leases_live
    ON maintenance_leases (lifecycle) WHERE lifecycle = 'live';
