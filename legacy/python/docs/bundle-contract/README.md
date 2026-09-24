# Portable bundle v1 contract

Generated contract artifacts are reproducible with:

```sh
uv run python -m bot.infrastructure.bundle.generate_contract
uv run python -m bot.infrastructure.bundle.generate_contract --check
```

`example.json` is synthetic. The bundle carries retained schedule/history and active
Thursday–Wednesday delivery bindings only. Memory and secrets are never sections;
the manifest reports memory preflight counts only. An indeterminate delivery attempt,
pending amendment, or queued/running rescan is invalid until reconciled or retired.

## Normative semantic rules beyond JSON Schema

Schema validation is necessary but insufficient. `Bundle` enforces the pinned
timezone/reset calculation, current-week-only delivery bindings, amendment-card
message/channel identity, and grouped native delivery references. Each bound attempt
has one `(channel_id, message_id)` and nonempty typed targets; every delivered current
binding resolves exactly once to a group and must match that message (and its channel
when the binding records one). Bound kinds are exactly `reminder`, `digest`,
`decline`, `card`, and `debug_card`; every target must have the same native family.
Reminder and card sends may group multiple native rows, while digest, decline, and
debug-card sends have exactly one. Retired native attempts may retain deleted native
references and must likewise use their matching family when targets are present.
`pre_journal_attestation` is the sole non-native retired kind and must be targetless.
It and all retirements require a nonempty bounded reason and a non-content actor
identifier: a Discord user Snowflake, `admin`, or `system`. Generic and memory
attempts are not portable; unresolved ones block a future export before projection.
Sent/message partial reminder rows and partial or unresolved decline rows are rejected
rather than inferred. Genuinely unsent reminders have neither field and no group.
Snowflakes are canonical nonzero decimal strings of at most 20 digits; padded numeric
aliases are rejected before identity checks. Format v1 remains unpublished and is not
compatible with released versions. General schedule foreign-reference checks are a
required later codec/preflight gate; this schema-only package does not claim to
implement them.

## Archive envelope and preflight

The v1 artifact is a deterministic `ZIP_STORED` archive with fixed member order and
1980 timestamps: `manifest.json`, then `schedule.json`, `config.json`,
`history.json`, `catalog.json`, `delivery.json`, and `rate_overrides.json`.
`manifest.json` follows `archive-v1.schema.json`; it contains `archive_version: 1`,
the existing domain manifest as `bundle_manifest`, and a SHA-256 plus exact UTF-8
byte size for every payload member. The manifest does not checksum itself; these
checks are integrity checks, not authentication.

Archive decoding accepts at most 8 MiB input, seven members, 8 MiB per member and
24 MiB total expanded bytes. Members must be unique regular UTF-8 JSON files with
stored compression, no links, directories, encryption, traversal or unknown names.
JSON rejects duplicate keys and non-finite numbers. No ZIP member is extracted to
the filesystem.

Pure preflight requires the destination guild, catalog revision/tokens, timezone and
reset time. It rejects mismatched context, duplicate natural keys and retained
schedule/history/delivery references before any database operation. Active schedule
references require destination catalog tokens. Only terminal run/amendment history
may use an explicitly listed source `catalog.historic_tokens` token; unknown tokens
are never silently discarded. Confirmed amendments are committed historical facts;
they may use explicit historic tokens. A deleted fixed timing may be referenced only
by a done/cancelled retained run. Amendment evidence must name a retained message,
and RSVP/decline users must be run participants. Secure publication has no default
path: callers must explicitly approve an existing owner-only (`0700`) private root.
It opens every directory by anchored no-follow descriptor and publishes with an
atomic hard-link no-replace operation. Absolute paths are traversed from an anchored
`/` descriptor one component at a time, so Linux and macOS reject symlinks without
requiring private ancestor directories; unsupported POSIX primitives fail closed.
Output is mode `0600` and only its own temporary file is removed on failure.

Fix amendments have two producer forms. Extractor fixes have no `op` and may
carry only weekday/time (or neither when unresolved), plus `also_mentioned`
annotations. Chat fixes have `op: edit|remove` and require `fixed_run_id` and
`weekly_when`; a partial chat identity must not fall back to the extractor form.
These kind-dependent rules are enforced by `Bundle` semantic validation.
