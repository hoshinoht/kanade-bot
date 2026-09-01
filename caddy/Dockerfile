# The stock caddy image cannot do DNS-01 against Cloudflare: DNS providers are
# compile-time plugins. This is the upstream-documented builder pattern, pinned
# to the same version the plugin combo was first verified against (2026-09-01).
FROM caddy:2.11.4-builder AS builder
RUN xcaddy build --with github.com/caddy-dns/cloudflare

FROM caddy:2.11.4
COPY --from=builder /usr/bin/caddy /usr/bin/caddy
