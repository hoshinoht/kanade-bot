# Boss knowledge tooling

Offline helpers for `boss/knowledge/`. Python 3.12+, standard library only for
`fetch.py`. `validate.py` needs PyYAML and jsonschema, which `uv` supplies
ephemerally.

## fetch.py

Downloads each public guide's `export?format=txt` into the git-ignored
`data/research/boss-guides/<boss>.txt`, and records `url`, `fetched` and
`sha256` in `index.json` there. The guide list (currently iSIingGunz's seven
Google Docs) lives in `GUIDES` in the script.

```sh
python3 scripts/boss_knowledge/fetch.py            # all guides
python3 scripts/boss_knowledge/fetch.py limbo kai  # a subset
```

Cached text is for local extraction and the anti-copy guard only. Never commit
it or paste it into tracked files.

## validate.py

```sh
uv run --no-project --with pyyaml --with jsonschema scripts/boss_knowledge/validate.py
```

- Validates `_meta.yaml` and every boss file against `boss/knowledge/schema.json`
  (unknown keys rejected) and rejects duplicate YAML keys.
- Checks semantics: the file stem matches `boss`, URLs, difficulties and HP
  phases are unique, dates are real and not in the future, and difficulties and
  `difficulty_notes` exist in the boss catalog (`--catalog`, default
  `boss/bosses.yaml`). Non-catalog bosses must declare `event`.
  Phase names and mechanic titles are unique, a phase `group` covers adjacent
  phases only and all of them set the same `cycle` (which needs a group), a document has at most one
  mission per series, and no two bosses share an `order` in a mission series
  (every boss involved fails). These checks run only on documents that pass the
  schema, so a malformed document reports its schema errors instead.
- Anti-copy guard: fails any text value (at any depth, so item titles, text and
  `detail`, phases, missions and mechanic labels and notes too) that shares at
  least `--ngram` (default 12) consecutive words with a cached guide, and reports each file's longest
  shared run. It is skipped with a warning when the cache is missing.

Exit status is non-zero if any file fails.
