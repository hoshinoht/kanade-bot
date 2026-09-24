# Pre-upgrade maintenance snapshots

This helper is a pre-DDL escape image for the v4 maintenance migration. The
caller supplies the already-open SQLite connection and must call
`upgrade_snapshot(connection, db_path)` before executing upgrade DDL. It uses
SQLite's online backup API; it never copies the live database file and never
prunes an older image. If the caller's DDL later fails, every previously
published image remains available.

## Filesystem policy

The source parent is opened from `/` one component at a time with
`O_DIRECTORY | O_NOFOLLOW`. The final source parent must be owned by the
current UID and have neither group-write nor world-write permission. A
user-owned ancestor is held to the same no-group/world-write rule. Root-owned
ancestor directories are trusted as standard path infrastructure when the
process can traverse them; a root-owned writable ancestor must be sticky, and
no ancestor is chmod-ed. On Darwin, `/var` and `/tmp`
are accepted only as root-owned symlinks to their exact `/private/var` and
`/private/tmp` targets, which are rewritten to those target paths before
no-follow traversal. Other symlinked components are rejected.

`maintenance-upgrades` is created or opened relative to the retained source
parent descriptor. It must be a current-UID directory with exact mode `0700`.
An existing directory with another owner or mode is rejected, never repaired
with `chmod`. The source-parent descriptor is fsynced immediately after that
directory is validated and before any temporary file is reserved, even when the
directory already exists: it may remain after a previous sync failure.
A failure at that sync aborts before SQLite
backup or caller DDL. A hidden random temporary file is then reserved with
`O_CREAT | O_EXCL | O_NOFOLLOW` and mode `0600`; its descriptor remains open
through SQLite backup and publication.

Python's SQLite interface does not provide a portable Darwin/Linux way to open
SQLite from that retained descriptor. The helper therefore uses the verified
temporary pathname for SQLite's destination and read-only validation, comparing
the pathname's device/inode with the retained descriptor before opening,
immediately after opening, after backup, and before publication. It requests
`journal_mode=DELETE`, closes SQLite, fsyncs the retained file, and checks the
read-only image before publication.

Publication is anchored to the maintenance directory descriptor: a unique
final name is made with a no-replace hard link, the temporary name is unlinked
only after it still names the expected inode, and the directory is fsynced.
Collisions never overwrite an existing file. Cleanup only attempts artifacts
whose inode was reserved by this invocation; an unknown replacement is left
untouched. Parent and maintenance-directory identities are checked again
before returning.

These checks are a local filesystem boundary, not protection from a hostile
same-UID or root process that can race a pathname between the separate checks,
rename retained directories, or open arbitrary SQLite clients. SQLite's
pathname API makes an FD-only claim impossible here; the deployment must keep
the snapshot parent private from such actors.

Errors are `SnapshotError` instances with short non-path-bearing codes. The
helper does not expose database paths, SQLite content, or raw filesystem error
text.
