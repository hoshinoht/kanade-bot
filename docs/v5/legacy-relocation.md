# Legacy Python relocation

The frozen v4 rollback implementation is `legacy/python/`. Its tracked manifest
is `legacy/python/RELOCATION_MANIFEST.txt`: 343 pure moves and one Docker
context hardening change, all recorded with pre-relocation git-index SHA-256.
`LICENSE` is retained at the repository root and copied into the legacy package
for its wheel metadata.

## Run and build

Run all v4 commands from `legacy/python/`: `uv sync --locked`, `uv run pytest -q
-m "not live_model"`, `uv run python -m bot.portal_styles --output /tmp/portal.css`,
and `docker build -f deploy/Dockerfile .`. Container files live in
`legacy/python/deploy/` (Dockerfile, `Dockerfile.dockerignore`, `compose.yaml`).

## Manual private mounts

No ignored or untracked deployment state moved. `deploy/compose.yaml` reads the
private `legacy/python/.env` (moved from the root on 2026-09-26) and mounts root data, config, persona and boss artwork paths; set
`KANATA_API_KEY_HOST_FILE` before any Compose command. Caddy is no longer part
of kanade: TLS ingress is the shared edge over `kanade_edge`. The private v4
persona layout lives in `config/personas-v4/` (mounted writable at
`/app/config/personas`); root `config/personas/` holds the v5 layout. Existing root private paths remain untouched; copy or mount
them only in an explicitly authorized deployment operation. The rollback image
must be built with `legacy/python/` as its context, never the repository root.
