# Persona vectors

Oracle captures of the v4 prompt assembly, for the later v5 prompt-format step.
Run from `legacy/python/`:

```sh
uv run python -m scripts.v5_vectors.persona
uv run python -m scripts.v5_vectors.persona --check
```

Inputs are public only: the tracked v4 Kanade templates
(`legacy/python/config/personas/personas/kanade/{identity.md,default.md,staging.yaml}`),
the code-owned policy prompts under `legacy/python/bot/chat/prompts/`, and
synthetic reply profiles written in `cases.py`. No private persona, profile, or
deployment file is read. There are no model, Discord, or network calls.

`bundles` records the raw template text. Before replaying, the generator
checks that the tracked v5 bundle `config/personas/bundles/kanade.yaml` holds
the same bytes (identity, behaviour prompt, staging) with no `voice`, so the
v5 bundle and this oracle cannot drift apart. Its v5-only `compact` section
has no v4 oracle and is pinned by the Rust tests instead. The Rust
`persona` test target checks that every case names a tracked bundle and that
the Rust loader yields the same text.

Each case gives a fixed aware clock, model name, current-card text, and an
optional synthetic profile: v4 profile Markdown plus optional partial staging.
The generator JSON-round-trips the document, then calls the v4 functions listed
in `provenance`, as `bot/chat/agent.py` does: it loads the bundle through
`load_example_bundle`, strips the profile Markdown as v4 does for stored
profiles, and assembles `component_system_prompt` and
`component_voice_reminder`. `expected` stores the exact strings: the rendered
header, runtime and focus lines, the full system prompt, the final voice
reminder, and the merged staging lines. Nothing is normalized or ignored.

The cases cover the bundle alone (named and unnamed model, with and without a
current card), a profile's voice and `Good` examples replacing the defaults,
the default voice when a profile has none, ignored placeholder voices and
examples, fenced/`Goodbye`/section-end rules, the round-robin example budget in
characters and in count, and partial and full staging overrides.

The Draft 2020-12 `schema.json` rejects unknown keys. The generator also checks
that case IDs are unique and that every `bundle_id` names a recorded bundle.
Any intentional change to voice or examples needs an approved update to these
vectors, not a looser comparison.
