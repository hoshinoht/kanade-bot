# v5 personas

```text
catalog.yaml            live catalog, private (git-ignored)
catalog.example.yaml    catalog template
bundles/<id>.yaml       one complete persona per catalog ID
bundles/kanade.yaml     tracked trusted fallback
profiles/<id>.yaml      optional reply profiles
profiles/example.yaml   profile template, never selectable
```

Only this README, the catalog example, the Kanade bundle, and the profile
example are tracked. Live catalogs, bundles, and profiles stay private. The v5
runtime reads only this layout; it never probes the v4 files under
`legacy/python/config/personas/`.

## Files

Every file is strict YAML with `schema_version: 1`. Unknown or duplicate keys,
invalid UTF-8, symlinks, non-regular files, and paths outside this directory
are rejected. Block scalars (`|`) keep their internal newlines.

- **Catalog:** `default` plus `personas`, a list of `{id, label, aliases}`.
  IDs are lowercase slugs (`a-z`, `0-9`, `-`, at most 50 characters). The
  bundle path is always `bundles/<id>.yaml`; paths are never configurable.
  Aliases are bare tokens (no slashes, `.`/`..`, or control characters) that
  must not collide with any ID or other alias. They are resolved only when
  importing or configuring a selection; the stored selection is the ID.
- **Bundle:** `id` (matching the filename), `identity`, `behaviour` with a
  required `prompt` and optional one-line `voice`, a complete `staging` map
  (`schedule`, `guide`, `guide_named`, `write`, `generic`), and optional
  `compact: {header_rewrite}`. The tracked Kanade bundle carries
  `header_rewrite`: v5-only text for rewriting a compact header line, with no
  v4 counterpart.
- **Profile:** `id` (matching the filename), `label`, optional `voice`,
  `prompt`, and optional partial `staging`; missing staging keys come from the
  selected bundle.

## Selection and fallback

At startup the configured persona is used if it is in the catalog and its
bundle validates, then the catalog default, then the tracked Kanade bundle.
If none validates, chat is disabled and no model is called. An explicit
switch or reload that fails keeps the saved selection and the running persona.

A member's profile is the first readable profile assigned to one of their
roles, then their saved profile if it is still offered, then none. An
unavailable saved profile is kept, not cleared. Profile roles never grant chat
access. An invalid profile is ignored as a whole.
