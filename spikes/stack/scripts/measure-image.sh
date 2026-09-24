#!/bin/sh
# Comparable size/startup/RSS evidence for the locked spike image.
# Reports health-ready latency and idle container RSS; the container is
# removed afterwards. No production config, no persisted volumes.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
name=kanade-stack-measure-$$
port=55335
pg_image=postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24
trap 'docker rm -f "$name" >/dev/null 2>&1 || true' EXIT
cd "$root"
docker build -t kanade-stack-spike:local .
printf 'app_image='; docker image inspect kanade-stack-spike:local --format 'id={{.Id}} size={{.Size}} bytes arch={{.Architecture}} os={{.Os}} user={{.Config.User}}'
printf 'postgres_image='; docker image inspect "$pg_image" --format 'id={{.Id}} size={{.Size}} bytes arch={{.Architecture}} os={{.Os}}'
printf 'host_binary_bytes='; wc -c < target/release/kanade-stack-spike
printf 'host_gzip_bytes='; gzip -c target/release/kanade-stack-spike | wc -c
# The Linux binary measure comes from extracting the image binary; see probes.
start=$(date +%s)
docker run -d --rm --name "$name" -p "127.0.0.1:${port}:3000" kanade-stack-spike:local >/dev/null
for n in $(seq 1 20); do
  if curl --fail --silent "http://127.0.0.1:${port}/healthz" >/dev/null; then break; fi
  sleep 1
done
curl --fail --silent "http://127.0.0.1:${port}/healthz" >/dev/null
ready=$(date +%s)
printf 'health_ready_seconds=%s\n' "$((ready - start))"
sleep 5
printf 'idle_rss='; docker stats --no-stream --format '{{.MemUsage}}' "$name"
docker kill --signal TERM "$name" >/dev/null
docker wait "$name" >/dev/null 2>&1 || true
