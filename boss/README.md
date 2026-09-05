# Boss data

`bosses.yaml` is the canonical catalog for boss identities, supported
difficulties, aliases, guide colours, and assets. Portraits are in
`portraits/`; entry artwork is in `artwork/entry/`; both are resolved relative
to the catalog.

Structured, source-backed knowledge is deliberately separate under `knowledge/`:
one lowercase YAML file per catalog boss plus `_meta.yaml`. When the chatbot is
configured, coverage is strict: every catalog boss must have exactly one
matching knowledge file, with no extras.

The catalog and knowledge are loaded at startup, so restart the bot after
changing either. Assets are served directly from this bind-mounted directory,
so portrait and entry-art changes appear on the next page load without a
restart. Asset READMEs and the catalog/knowledge are tracked; game images are
git-ignored.

Guide prose remains in `config/guide.yaml`. `bossctl guide` derives its boss
entries from this catalog (or an explicit `bossctl guide --bosses PATH` catalog).
