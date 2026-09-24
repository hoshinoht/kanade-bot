# Scheduler vectors

This producer-side slice records stateful v4 scheduler behavior through a real
in-memory `Repo`: materialisation, matching one-off adoption, fixed-run edit and
retirement, reminder rows, and RSVP reactions. Run from `legacy/python/`:

```sh
uv run python -m scripts.v5_vectors.scheduler
uv run python -m scripts.v5_vectors.scheduler --check
```

Each case serializes its aware clock, timezone/reset configuration, deterministic
UUID source sequence, owner/member/channel IDs, and operations. The generator
JSON-round-trips documents before invoking the replayer; the replayer accepts one
case, validates its input before opening a store, patches the `db.new_id`,
`db.utcnow`, and imported `materialise.utcnow` seams only for that invocation,
and opens one clean in-memory store. Tests JSON-round-trip cases and replay each
twice to prove clean-store byte-identical output.
IDs are retained as meaningful state rather than normalized away.

Snapshots preserve ordered fixed runs, runs, reminder rows, RSVPs, and recorded
side effects (none in this persistence-only slice). They intentionally omit
SQLite `created_at` bookkeeping only; it does retain RSVP `source` and aware
`at`, because they are portable scheduler facts. Reminder rows are included
because they are a materialisation consequence. Delivery, mention policy,
digest, and Discord transport effects are separate future slices and are not
faked here.

The checked-in Draft 2020-12 schema discriminates every operation, constrains
status/source/state domains, types final rows, and rejects unknown keys. Its
`replayCase` definition requires only `case_id` and `input`, while `case` adds
the completed `expected` result. Runtime replay applies that same input schema
with `Draft202012Validator` and a `FormatChecker` before opening its store;
`validation.validate_document` applies the complete document schema before its
semantic gate. Static validation accepts a materialized key only after its
fixed timing has been declared and materialization requested; replay then
accepts it only if the v4 oracle actually produced that run. It checks
reference ordering, exact fixed-edit field sets, expected-step cardinality,
and operation/result pairing that JSON Schema cannot express across arrays.
Each step has exactly one success value or the declared exact error type;
expected outputs come only from replaying the v4 oracle.
