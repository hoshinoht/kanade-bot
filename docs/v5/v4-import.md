# `kanade import v4` (testing import)

Status: implemented in `src/import/v4/` (CLI `src/cli/mod.rs`, config
`src/runtime/config/import.rs`); tests `tests/import/main.rs`.

A one-off import so the admin app can be tested on real data. It is a
**narrow, explicit exception** (user decision 2026-09-25) to the "no raw v4
SQLite import" rule in `compatibility.md`: it reads a read-only *copy* of the
v4 database, never the live file, and it is not the portable-bundle import.

## What it imports

- **Weekly fixed runs** — all of them, whatever their age. Each goes through
  `SchedulerService::add_fixed_run` (the normal write path) under its **v4 id**
  (v5 runs in the same guild, so ids stay), attributed to `system:import` on
  surface `import` with request id `v4-fixed:<id>`: one history record per
  timing, so blame and rollback work per timing and a re-run is refused by
  the request id. Rows are validated first and skipped with a reason code:
  `bad_id` (not a lowercase UUID), `bad_owner`, `bad_channel` (not a
  snowflake; a missing channel is kept as none), `bad_bosses`,
  `unknown_boss` (not an exact canonical token of the loaded catalog),
  `bad_weekday` (0 = Monday … 6), `bad_time`, `bad_participants` (not
  snowflakes), `no_participants`. Participants and channels are checked for
  shape only: the import has no Discord roster or watched-channel list, and
  v4 accepted these rows. After the timings, the current and next boss weeks
  are materialised as `serve` does (one more record; nothing when the runs
  exist).
- **Chat and extraction logs** at or after the cutoff: the 90-day log
  retention (`DEFAULT_LOG_RETENTION`), or guild-local midnight of `--since`
  when later. Ids become `v4-<v4 id>`, which marks them and makes re-runs
  idempotent. Rows with an unreadable timestamp are skipped
  (`bad_timestamp`), older ones counted as `outside_window`.
  - Chat: `answered` → `answered`; `failed` with an error → `error`; anything
    else (a `failed` empty reply) → `unknown`. Timestamp, channel, message,
    author, question, reply, error, latency/model/tools ms, tokens and
    `rounds` (as the request count) are kept. v4 `model_rounds` become v5
    rounds (model = the v4 model, `response` = the round's content, `tools` =
    its requested tools, or the names of the calls placed in it, deduplicated
    in call order, when v4 left `requested_tools` empty or missing) carrying
    the v4 tool-call entries of that round (their `output` and `ms` show as
    the turn's `result`/`took_ms`); with no recorded rounds, one round
    carries all calls. Reasoning effort,
    finish reason, per-round latency, bundles and guardrails stay empty.
  - Extraction: amendments present → `proposed` (their ids kept as
    `proposal_ids`; they resolve to no v5 proposal and show as `missing`);
    none with a JSON response → `no_change`; unreadable ids or a non-JSON
    response (v4 logged failures as error text) → `unknown`. Timestamp,
    model, prompt, raw response, latency and message ids are kept; the
    channel (when all read messages share one) and members come from the
    referenced messages; request count 1.
- **Watched messages referenced** by those logs and present in the snapshot
  (`not_in_snapshot` otherwise). They are stored processed (v4's time, else
  the import's), so the v5 extractor never treats them as new backlog.
  Messages already in the v5 cache are left alone.

Not imported: runs, RSVPs, reminders, v4 audit/history, amendments (never as
drafts or proposals), members, memory, rate limits, rescan jobs, config.

## Running it

The import takes the store's owner lock, so it cannot run while `serve` owns
the store: stop the v5 container first. It needs `KANADE_TIMEZONE`,
`KANADE_DB_PATH`, `KANADE_OWNER_LOCK_DIR` (an existing private directory, as
for `serve`) and `KANADE_CATALOG_FILE` (default `boss/bosses.yaml`).

```sh
kanade import v4 --from /import/v4.sqlite                    # dry run
kanade import v4 --from /import/v4.sqlite --since 2026-09-01 # narrower logs
kanade import v4 --from /import/v4.sqlite --apply            # write
kanade import v4 --from /import/v4.sqlite --refresh-logs     # dry run of a log refresh
kanade import v4 --from /import/v4.sqlite --refresh-logs --apply
```

A **dry run is the default**: it prints what would be added, what is already
present and what is skipped (by reason), and writes nothing; when the store
does not exist yet it is not created. The output has counts, reason codes
and v4 fixed-run ids only, never message text or member names. Run it with
`--apply` once the counts look right; running `--apply` again adds nothing.

`--refresh-logs` repairs logs imported by an older mapping (2026-09-26:
rounds whose v4 `requested_tools` was empty lost their tool names, so
`tools_used`, the `chat_tools` index and the Tools filter missed them).
Because a normal re-run skips ids already present, it instead rewrites
every in-window chat and extraction log from the snapshot, replacing the
stored `v4-<id>` row (with its rounds and `chat_tools`/`extraction_members`
index rows) and adding any not yet imported, all in one write transaction.
Only `v4-` ids are written (the store refuses any other id); native v5
logs, fixed runs, materialised runs, messages and everything else are not
touched. Its dry run reports how many rows would be replaced and added;
applying it again replaces the same rows with the same content.

The snapshot is opened `mode=ro` with `immutable=1`: SQLite neither writes
nor locks it and creates no `-wal`/`-shm` files, so it must be a quiescent
copy (an online backup, e.g. `sqlite3 kanade.db ".backup snapshot.sqlite"`),
never the live v4 file. `--from` naming the v5 store itself is refused.
Container usage is in `deploy/README.md`.
