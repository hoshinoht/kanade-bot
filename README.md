# Kanade v5

The Rust v5 rewrite is in progress. The v4 (Python) implementation is gone
from this repository; it remains in git history up to `487c4ed`.

## v5 runtime bootstrap

The current Rust binary intentionally provides only an offline development
health server. See [`docs/v5/runtime-bootstrap.md`](docs/v5/runtime-bootstrap.md)
for commands, configuration, and unavailable product capabilities. Container
files live in the root `deploy/` directory; the deploy runbook is
[`deploy/README.md`](deploy/README.md).

## License

Copyright (c) 2026 hoshinoht. The new Rust release and its supporting code are
licensed under the [GNU General Public License, version 3 only](LICENSE)
(`GPL-3.0-only`), without warranty.

The v4 Python tree, available in git history, retains its MIT
license. Third-party components retain their own
licenses; this change does not alter licenses granted for earlier releases.
