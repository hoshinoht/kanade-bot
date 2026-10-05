# Kanade v5

The Rust v5 rewrite is in progress. The frozen v4 (Python) rollback
implementation is no longer part of this repository: it is kept locally under
the git-ignored `legacy/python/` and remains in git history up to `487c4ed`.

## v5 runtime bootstrap

The current Rust binary intentionally provides only an offline development
health server. See [`docs/v5/runtime-bootstrap.md`](docs/v5/runtime-bootstrap.md)
for commands, configuration, and unavailable product capabilities.

## v4 rollback

Operators with a local `legacy/python/` (restore it with
`git archive 487c4ed legacy/python | tar -x`) run its commands from there.
v5 container files live in the root `deploy/` directory; the deploy runbook is
[`deploy/README.md`](deploy/README.md).

## License

Copyright (c) 2026 hoshinoht. The new Rust release and its supporting code are
licensed under the [GNU General Public License, version 3 only](LICENSE)
(`GPL-3.0-only`), without warranty.

The Python rollback tree, available in git history, retains its MIT
license. Third-party components retain their own
licenses; this change does not alter licenses granted for earlier releases.
