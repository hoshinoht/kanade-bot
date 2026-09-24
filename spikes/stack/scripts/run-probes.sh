#!/bin/sh
# Reproducible store/capability probes. Uses only loopback networking,
# synthetic credentials, a pinned throwaway Postgres image and temp dirs.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work=${TMPDIR:-/tmp}/kanade-stack-spike-$$
name=kanade-stack-pg-spike-$$
port=55432
image=postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24
mkdir -p "$work"
trap 'rm -rf "$work"; docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
cd "$root"
cargo run --locked --release -- capabilities
cargo run --locked --release -- sqlite "$work/store.sqlite"
docker run --rm --name "$name" -p "127.0.0.1:${port}:5432" -d \
  -e POSTGRES_DB=kanade_spike -e POSTGRES_USER=spike -e POSTGRES_PASSWORD=synthetic-only \
  "$image" >/dev/null
for n in $(seq 1 30); do
  if docker exec "$name" pg_isready -U spike -d kanade_spike >/dev/null 2>&1; then break; fi
  sleep 1
done
docker exec "$name" pg_isready -U spike -d kanade_spike >/dev/null
base="postgres://spike:synthetic-only@127.0.0.1:${port}"
cargo run --locked --release -- postgres "$base/kanade_spike"
# Logical native backup (custom format) and restore into a fresh database.
docker exec "$name" pg_dump -U spike -d kanade_spike -Fc > "$work/dump.pgdump"
docker exec "$name" createdb -U spike -T template0 kanade_spike_restore
docker exec -i "$name" pg_restore -U spike -d kanade_spike_restore < "$work/dump.pgdump"
cargo run --locked --release -- postgres-verify "$base/kanade_spike_restore"
printf 'pg_dump_bytes='; wc -c < "$work/dump.pgdump"
