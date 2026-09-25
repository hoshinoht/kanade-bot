# V5 contract vectors

`domain/` is the first producer-side subset of the v5 freeze.  It captures pure
v4 `weeks`, `timeutil`, `ids`, and `bosses` behavior as language-neutral JSON.
Run it from `legacy/python/`:

```sh
uv run python -m scripts.v5_vectors
uv run python -m scripts.v5_vectors --check
```

The generator first JSON-round-trips the serialized inputs and fixtures, then
replays each case through one dispatch function that calls the v4 oracle.
Expected values are never hand-authored. It writes only
`docs/v5/vectors/domain/{weeks,timeutil,ids,bosses}.json`.

The catalog and UUID fixtures are synthetic and checked in. Difficulty and boss
definitions are ordered arrays, not semantic JSON objects. The replayer rebuilds
the v4 maps from those arrays, preserving the contractual `n`, `h`, `x` order in
user-visible valid-form/error text without depending on JSON object order.
Inputs use RFC 3339 aware timestamps where an instant matters.
Normalization preserves offsets, seconds, ordering, and full stable error
messages. Datetimes and `time` results become ISO strings and tuples become
arrays. No nondeterminism is ignored.

Provenance is recorded in each file: invoked v4 functions, actual test refs
where direct tests exist, and retained inventory surfaces. `timeutil` has no
direct test function, so its provenance intentionally lists no invented test
reference. The schema discriminates family/operation, rejects extra input and
result fields, and requires either a typed error with a declared exact class or
a typed success value. It is intentionally a small producer contract, not a
generic comparison framework.

Covered here: reset boundaries, calendar-vs-boss week arithmetic, fixed slots,
DST wall-clock week ends, weekday/time parsing, ISO conversion, ID display and
resolution errors, and synthetic catalog token/reference/list/name/description
parsing. Bare weekday-forward behavior belongs to `api.service.parse_when` and
chat tooling rather than these four pure domain modules, so it is pending its
later API/chat vector slice. Scheduling/materialization, RSVP, notifications,
extraction, chat, wire/delivery, and portable bundle vectors are also pending.

`scheduler/` is the next stateful producer subset: real in-memory v4 `Repo`
replay for materialisation, fixed-run edits/retirement, reminder rows, and RSVP
reactions. See its [README](scheduler/README.md); run it with
`uv run python -m scripts.v5_vectors.scheduler --check` from `legacy/python/`.

`persona/` captures the v4 prompt assembly from public Kanade templates and synthetic
profiles; see its [README](persona/README.md) and run
`uv run python -m scripts.v5_vectors.persona --check`.

`extract/` freezes the v4 extraction oracle: keyword gate, burst windows,
day/time resolution, run matching, merge, prompt and structured-output schema
bytes, response parsing with retry/quarantine over a scripted model, burst
planning, commit outcomes and proposal cards, one schema per family listed in
its `index.json`. See its [README](extract/README.md) and run
`uv run python -m scripts.v5_vectors.extract --check`.
