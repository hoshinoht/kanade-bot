# Maintenance foundation (not an operational release)

Schema v14 records durable maintenance state, operation leases and delivery
attempt metadata. A fresh empty store is `OPEN` with adoption complete. A v9–v13
store receives a unique, non-pruning SQLite online backup before any upgrade DDL,
then starts `BLOCKED` with adoption pending.

One writable `Repo` retains a separate device/inode-keyed lockfile in the
operator-configured `DB_OWNER_LOCK_DIR` for its lifetime; aliases contend and
fork-inherited repositories are unusable. The root is an absolute, existing
mode-`0700` directory shared by the cooperating service UID. Native and private
Compose mounts must provide the same root to every writer; this foundation does
not claim a deployment mount is configured. It does not exclude arbitrary
SQLite clients or hostile same-UID/root filesystem actors. Live history
recording/export must later use the running owner's authenticated lease-gated
API, not a second writable repository.

The upgrade snapshot is an old-image escape only before accepted v14 writes. It
does not replace the forthcoming bundle-based rollback path, and snapshots are
never pruned by this feature.

The deny-by-default SQLite authorizer is installed before recovery or upgrade
writes. It protects `maintenance_state`, `maintenance_leases`, and the delivery
ledger in every mode; only fixed-purpose internal authorities may update those
tables. Existing v4 repository methods remain covered only in `OPEN`, while an
admitted task-bound operation may finish ordinary DML after admission closes.
Child work uses a durable reservation created before task spawn and an explicit,
single-use transfer; a reservation is never itself a write authority.

Prepare timeout or cancellation enters `BLOCKED` without revoking accepted work.
A successful drain reports quiescence while remaining `PREPARING`; it never
claims `FROZEN` or fabricates reconciliation fields. Restarted operations are
retained as timestamped, reasoned orphans; retirement is an actual
task-bound operation, while taskless recovery writes exist only inside the
constructor's still-live bootstrap window. A blocked store containing only
those preserved rows may close without erasing them. Complete runtime ingress,
Discord reconciliation, delivery journaling, export/import, and operator
controls are later work. Do not treat v14 as production-releasable or run it
against a live store.
