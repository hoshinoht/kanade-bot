# Kanade v5

The Rust v5 rewrite is in progress. The frozen v4 rollback implementation is
independently runnable from [`legacy/python/`](legacy/python/).

## v5 runtime bootstrap

The current Rust binary intentionally provides only an offline development
health server. See [`docs/v5/runtime-bootstrap.md`](docs/v5/runtime-bootstrap.md)
for commands, configuration, and unavailable product capabilities.

## v4 rollback commands

```sh
cd legacy/python
uv sync --locked
uv run pytest -q -m "not live_model"
uv run python -m bot.portal_styles --output /tmp/portal.css
docker build -f deploy/Dockerfile .
```

v4 container files live in `legacy/python/deploy/`; v5 container files will live
in the root `deploy/` directory.

See [`legacy/python/README.md`](legacy/python/README.md) for the v4 product and
[`docs/v5/legacy-relocation.md`](docs/v5/legacy-relocation.md) for rollback
mount requirements.

## License

Copyright (c) 2026 hoshinoht. The new Rust release and its supporting code are
licensed under the [GNU General Public License, version 3 only](LICENSE)
(`GPL-3.0-only`), without warranty.

The independently licensed Python rollback tree retains its
[MIT license](legacy/python/LICENSE). Third-party components retain their own
licenses; this change does not alter licenses granted for earlier releases.
