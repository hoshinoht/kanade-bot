# v5 stack evaluation and bootstrap decision

**Status:** user selected **SQLite** after both-store evaluation. Independent review `ses_f37772941ffeVtwGdegwSYdu8h` passed the corrected spike; parent adopts the foundation below for production bootstrap, not as proof of completed runtime or release readiness. The root crate must recheck its own dependency features and advisory applicability.

## Platform and method

Measured 2026-09-22 on macOS arm64 (`rustc 1.98.1`, Cargo 1.98.1) with OrbStack Docker Linux/arm64, 1 vCPU and 1.93 GiB configured memory. Exact tool and image facts are in [stack-evidence/environment.txt](stack-evidence/environment.txt); the resolved 322-package graph is [cargo-metadata.json](stack-evidence/cargo-metadata.json). The frozen v4 image was inspected read-only, not rebuilt: `kanade-bot:latest` is 348,339,218 bytes uncompressed (arm64).

Both store probes use the identical two-row synthetic delivery ledger, frozen timestamps, guild 42, active delivery IDs, transactional insert, deliberate rollback, close/reopen, and JSON logical snapshot/restore. They do not read a v4 SQLite file or any private bundle. `cargo test` covers the same invariants (4 capability tests, SQLite store test, Postgres store test gated on `KANADE_STACK_PG_URL` with a skip message when unset).

## Candidate recommendation

Recommend a **Tokio + Axum + tower-http + Rustls (`ring` provider) + SQLx + chrono/chrono-tz + serde/serde_json** foundation, a normal glibc `debian:bookworm-slim` final image, and **Twilight 0.17.1** as the Discord gateway/HTTP candidate. Serve versioned PWA assets through Axum; compile-time `include_*` versus copied assets remains an application/PWA-cache decision.

* Axum 0.8 documentation demonstrates `axum::serve(...).with_graceful_shutdown` and `tower_http::services::ServeFile`; the spike served `/healthz` and a static fixture and accepted SIGTERM in an ephemeral non-root container, including a `--read-only --cap-drop ALL` run.
* Tokio supplies the single async runtime and signal handling. Rustls is selected through SQLx's `runtime-tokio-rustls` and pinned directly with the `ring` provider; Rustls documents TLS 1.2/1.3 support. The provider is installed once in application bootstrap (`src/tls.rs`, first in `main`, before any rustls builder use) and shared with tests through the same seam — no test-local setup stands in for it. The Linux-target feature graph (`cargo tree --target aarch64-unknown-linux-gnu -e features`, saved in evidence) shows rustls with both `ring` and `aws-lc-rs` enabled, which is why auto-detection is ambiguous and the explicit install is required. An offline loopback test completed a real TLS 1.3 handshake against a synthetic self-signed cert, and rejected the same server with an empty trust store.
* The `clients` subcommand exercises the production startup seam with no network: it asserts the bootstrap-installed process default exists, constructs the intended Twilight HTTP client (`twilight_http::Client::new`, no request sent), and parses representative SQLx Postgres options (never connects). Run as `docker run --rm --network none` on the Linux image it prints `tls_provider=ring discord_http_client=constructed pg_options=parsed`. The first such run exposed a real gap: Twilight's platform verifier needs OS CA roots and `debian:bookworm-slim` ships none, so client construction panicked; the final image installs `ca-certificates` (only addition) and the smoke passes.
* A local WebSocket upgrade/echo round-trip passed over loopback using `tokio-tungstenite` 0.28.0 (the same protocol family Twilight's gateway uses); no Discord endpoint was contacted.
* `chrono-tz` converted the fixed UTC instant to Toronto's repeated local 01:30 after the 2026 DST fallback, proving IANA time-zone data is available in the candidate.
* Twilight 0.17.1 gateway and HTTP crates compile in the locked graph without a token. Its current docs show a single `Shard` event loop and a default Rustls HTTP connector. Compared with Serenity 0.12, Twilight is lower-level and modular, which fits one guild/one writer and explicit retained delivery IDs; Serenity is the higher-level client/cache option. Serenity's docs show cache enabled by default and recommend intentional intent/cache configuration. Neither was connected to Discord.

Authoritative API references consulted: [Axum graceful shutdown](https://github.com/tokio-rs/axum/blob/main/examples/graceful-shutdown/src/main.rs), [Axum file services](https://github.com/tokio-rs/axum/blob/main/axum/src/docs/routing/route_service.md), [Tokio signals](https://tokio.rs/tokio/topics/shutdown), [SQLx transactions](https://docs.rs/sqlx/latest/sqlx/sqlite/type.SqlitePool.html), [Rustls README](https://github.com/rustls/rustls/blob/main/README.md), [Twilight gateway](https://github.com/twilight-rs/twilight/tree/main/twilight-gateway), [Serenity README](https://github.com/serenity-rs/serenity/blob/current/README.md), [cargo-audit install](https://github.com/rustsec/rustsec/blob/main/cargo-audit/README.md), [cargo metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html), [pg_restore](https://www.postgresql.org/docs/current/app-pgrestore.html), and [SQLite VACUUM INTO](https://sqlite.org/lang_vacuum.html).

## Store comparison

| Criterion | SQLite | PostgreSQL |
| --- | --- | --- |
| Same workload | PASS: `BEGIN IMMEDIATE`, transaction/rollback, restart, online `VACUUM INTO` snapshot restore, logical JSON snapshot | PASS: transaction/rollback, restart, logical JSON snapshot, native logical `pg_dump -Fc` (1,873 bytes) + `pg_restore` into a fresh database with identical rows/IDs and rollback |
| Single-guild writer | Natural one-file/WAL deployment; still enforce one process | Works, but adds a network service without improving the decided single-writer topology |
| Active delivery IDs/history | Fixture preserved both IDs exactly | Same fixture preserved both IDs exactly through dump/restore |
| Portable rollback fit | Backend-neutral logical snapshots; `VACUUM INTO` is an online consistent snapshot, not the v5 contract | Backend-neutral logical snapshots; `pg_dump`/`pg_restore` are logical native backups, not physical base backups and not the v5 contract |
| Operations/security | No DB credential or network listener; backup volume ownership remains | Synthetic loopback test required credentials, a listener, service health/start order, secret rotation, backup and resource policy |
| Measured runtime/image cost | Shared binary; SQLite database files only | Shared client binary; plus the 291,065,228-byte `postgres:17-alpine` service image and its volume |

**Decision: SQLite, explicitly selected by the user.** It satisfies the comparable probe without adding credentials, network exposure, or a second service whose image alone exceeds the application image. v5 still uses only the portable bundle migration contract, never the v4 live schema/file. PostgreSQL is evaluated but not adopted.

## Measurements and budgets for review

All from the pinned digest image on Linux/arm64 unless noted:

* Application image `kanade-stack-spike:local`: **120,201,708 bytes** uncompressed, arm64, runs as `kanade` (includes `ca-certificates`, required for Twilight's platform TLS roots).
* Linux glibc release binary extracted from that image (ELF aarch64, dynamically linked): **13,112,904 bytes**, gzip **4,491,228 bytes**. The macOS host binary (12,127,792 bytes, Mach-O) is a different platform/linker output and is not comparable.
* PostgreSQL service image `postgres:17-alpine@sha256:b0f95…300e552b24` (digest-pinned in both probe and measure scripts): **291,065,228 bytes**, reported separately as the cost of choosing PostgreSQL.
* Health-ready: loopback `/healthz` succeeded on the first poll after `docker run` (sub-second); idle container RSS after 5 s: **1.76–3.96 MiB** across runs (`docker stats`).
* The v4 image inspect size is **348,339,218 bytes**; it is not a comparable application-startup/RSS baseline because no safe fake v4 harness was available.

Do not turn spike numbers into release promises. Parent accepts these as the architecture-qualified bootstrap measurement baseline. Bootstrap records and explains size/resource differences against it; numerical full-product release limits are deferred until the representative Discord/config/store workload exists and remain a release gate. No cross-platform or unlike-workload regression ratio is an acceptance claim.

## Licensing, advisories, and security review

The captured spike metadata/license inventory below predates the user's switch
of the project and spike to `GPL-3.0-only`. Third-party licenses are unchanged;
retain this historical evidence and refresh the license inventory, compatibility
review and distribution notices for the final release graph.

* `cargo-audit 0.22.2` (pinned; installed with `--locked` to an isolated `--root` under the approved temp dir, not system-wide) against the RustSec advisory database (1,261 advisories loaded 2026-09-22) reports exactly one finding: `rsa 0.9.10`, RUSTSEC-2023-0071 (Marvin timing sidechannel, medium, no fixed upgrade). Applicability: `rsa` enters the all-targets lockfile resolve via `sqlx-mysql` (pulled by `sqlx-macros-core` offline support); it is absent from the host AND Linux-target build graphs (`cargo tree`, including `--target aarch64-unknown-linux-gnu`) and no `sqlx_mysql`/`rsa` artifact exists in the host target dirs. The spike performs no RSA or MySQL operations and uses only the sqlite/postgres paths. The finding is therefore unreachable in this spike, but stays open pending a production feature recheck: if the product ever enables the pulling SQLx features, the audit must be rerun against that feature set. Full output: [stack-evidence/cargo-audit.txt](stack-evidence/cargo-audit.txt); command exits 1 by design when findings exist.
* License identifier inventory over all 322 locked packages (`spikes/stack/scripts/license-review.py`, stdlib only): prints the verbatim expression of every package outside the common permissive set and requires a manually recorded note per identifier — it is an inventory aid, not a policy proof, and never auto-approves OR-choices. Manually recorded: `CDLA-Permissive-2.0` (Mozilla-root-derived `webpki-roots*` data, no copyleft obligation on the binary) and `LGPL-2.1-or-later` (appears only inside r-efi's `MIT OR Apache-2.0 OR LGPL-2.1-or-later`, so a permissive choice is available; r-efi is lockfile-only for non-host wasi targets and absent from host/Linux build graphs). No identifiers without a manual note. Report: [stack-evidence/license-review.txt](stack-evidence/license-review.txt).
* The spike uses synthetic credentials only, loopback-only bindings, no persisted volumes, per-run unique container names, and cleanup traps. Dockerfile `FROM` refs are digest-pinned (`rust:1.98-bookworm`, `debian:bookworm-slim`); the Postgres probe image is digest-pinned in the script.

## Gaps and owner choices

No production Discord, Kanata, Ollama, OAuth/cookies, or real Discord gateway handshake was run; an authenticated gateway/TLS validation belongs after the gateway adapter is authorized. The logical `pg_dump`/`pg_restore` rehearsal and `VACUUM INTO` snapshot are backend-native probes, not the pending portable-bundle contract. `cargo-deny` was not used; the license review above covers identifiers, not a deny-policy check. Store and bootstrap stack/glibc-with-CA direction are accepted; representative full-product budgets and actual adapter behavior remain later gates.
