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
  (`schedule`, `guide`, `guide_named`, `write`, `generic`), optional
  `compact`, optional `nudges` and optional `failures`. `compact` holds v5-only prompts (text of
  any length) that ask a small model to rewrite a single line: at least one
  of `header_rewrite` (reminder header lines) and `nudge_rewrite`
  (self-service nudge lead-ins). The tracked Kanade bundle carries
  `header_rewrite` only.
- **Profile:** `id` (matching the filename), `label`, optional `voice`,
  `prompt`, optional partial `staging` and optional `nudges`; missing staging
  keys come from the selected bundle.

Text values must be strings: unquoted numbers, booleans and nulls are
rejected, so quote them (`'5'`).

### Staging lines

Each line is one line of at most 200 characters after outer whitespace is
trimmed, with no mentions (`<@…>`, `<#…>`, `@everyone`, `@here`).
`guide_named` holds exactly one literal `{boss}` and no other braces; the other
lines hold no braces at all. The boss name is substituted literally; an unsafe
name or a result over 300 characters shows the `guide` line instead.

### Failures

A fixed line posted when the provider's content filter blocked an answer
(after the clean retry); the provider's own refusal text is never shown.
Without it the code-owned neutral line is used. The tracked Kanade bundle
declares none.

```yaml
failures:
  content_blocked: "Nope, not touching that one."
```

The section must declare the line: one line of at most 200 characters,
unpadded, with no mentions, links (including scheme-less Discord invites)
or braces. It is posted verbatim.

### Nudges

Lead-in lines for self-service tips. Code appends the action and link
(`→ edit the run: <link>` / `→ request a change: <link>`), so lines never
carry them.

```yaml
nudges:
  playful:            # default mood
    - "Eh? You want me to change it? ...fine, here's the button, do it yourself."
    - "I already did the hard part, you know. Tweaking it is on you~"
    - "Hmph. Everything you need is right here."
  gentle:             # after a failure or a frustrated message
    - "Ah... that didn't go well, did it. You can fix it here."
    - …
  request_form:       # optional pools for changes that need approval
    playful: [...]
    gentle: [...]
```

Every pool is optional but a section must declare one. Pools hold 3 to 20
lines of at most 140 characters, one line each, without padding, links, URLs
or mentions; the only placeholders are `{boss}`, `{day}` and `{time}`,
substituted literally. For a given purpose and mood, the member's profile
pools are used first, then the bundle's, then neutral built-in lines; a
missing `gentle` pool never borrows `playful` lines.

Lines rotate per channel without immediate repeats. When a `rewrite` model is
configured, the picked line (placeholders unfilled) may be rewritten by it,
guided by `compact.nudge_rewrite` and the effective voice (profile, then
bundle); the rewrite must pass the same line rules and keep exactly the
seed's placeholders, otherwise the seed line is used as written.

## Compiled prompt

The chat prompt is compiled from the selected bundle and profile: identity,
behaviour, profile prompt, `Good` examples (profile examples replace the
behaviour's), then the code-owned policies in `src/chat/prompts/`, the
per-turn clock/model/card lines and a voice cue. The effective voice is the
profile `voice`, then the bundle `voice`, then a built-in default; `<...>`
template values count as unset. The last message of every request is the
fixed scheduler voice reminder. Persona files cannot change tools, access or
policies, and file names, digests and selection sources never reach the
model.

## Selection and fallback

At startup the configured persona is used if it is in the catalog and its
bundle validates, then the catalog default, then the tracked Kanade bundle.
If none validates, chat is disabled and no model is called. An explicit
switch or reload that fails keeps the saved selection and the running persona.

A member's profile is the first readable profile assigned to one of their
roles, then their saved profile if it is still offered, then none. An
unavailable saved profile is kept, not cleared. Profile roles never grant chat
access. An invalid profile is ignored as a whole.
