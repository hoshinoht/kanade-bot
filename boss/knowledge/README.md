# v5 boss knowledge (schema v2)

Structured, source-backed boss knowledge for Kanade v5: concise Discord
explanations and the portal Bosses page. One lowercase YAML file per boss
(`<boss key>.yaml`) plus `_meta.yaml`, validated against `schema.json`.

The v4 rollback keeps its own frozen copy in `legacy/python/boss/knowledge/`
(schema v1), which the v4 container validates at startup. Do not edit that copy.

## Fields

Required, as in v1: `boss` (catalog key), `summary`, `core`, `danger`, `tips`
(1-8 bullets each, aim for 6 or fewer, 500 characters max), `sources`.

Optional: `difficulty_notes` (catalog letter to text), `notes`, and new in v2:

- `event`: `{name, availability}` for seasonal or event bosses that are not in
  the boss catalog (e.g. `kai.yaml`). A boss outside the catalog must declare it.
- `difficulties`: a list of `{name, entry_level, boss_level, pdr_percent,
  party_max, force: {kind: arcane|sacred, value}, hp: [{phase, value}],
  recommended_spec: {kind, text}, notes}`. Only `name` is required. `name` is
  `Easy|Normal|Hard|Chaos|Extreme` and must be a catalog difficulty. `hp.phase`
  is `'1'`, `'2-1'`, or `total` when phases are HP thresholds on one bar.
  Unit-bearing values stay strings (`241.5t`, `10.266q`).
- `sources` entries are objects: `{url (https), title, author, kind:
  guide|wiki|tool|official, fetched: YYYY-MM-DD, updated?: YYYY-MM-DD}`.

Unknown keys are rejected at every level.

## Attribution and copying

Guide-derived entries hold our own concise paraphrase and the structured facts
only, and credit the author in `sources`. Never paste guide prose: this
repository is public. Raw guide text is cached only in the git-ignored
`data/research/boss-guides/`, and `validate.py` fails any text that shares 12
or more consecutive words with a cached guide.

## Tooling

See `scripts/boss_knowledge/README.md`:

```sh
python3 scripts/boss_knowledge/fetch.py            # refresh the local guide cache
uv run --no-project --with pyyaml --with jsonschema scripts/boss_knowledge/validate.py
```

`REVIEW.md` records per-boss changes, conflicts and open questions from the
most recent import.
