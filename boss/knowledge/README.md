# v5 boss knowledge (schema v2)

Structured, source-backed boss knowledge for Kanade v5: concise Discord
explanations and the portal Bosses page. One lowercase YAML file per boss
(`<boss key>.yaml`) plus `_meta.yaml`, validated against `schema.json`.

The v4 rollback keeps its own frozen copy in `legacy/python/boss/knowledge/`
(schema v1), which the v4 container validates at startup. Do not edit that copy.

## Fields

Required, as in v1: `boss` (catalog key), `summary`, `core` (unless `phases`
is present), `danger`, `tips` (1-8 bullets each, aim for 6 or fewer, 500
characters max), `sources`.

Optional: `difficulty_notes` (catalog letter to text), `notes`, and new in v2:

- `event`: `{name, availability}` for seasonal or event bosses that are not in
  the boss catalog (e.g. `kai.yaml`). A boss outside the catalog must declare it.
- `difficulties`: a list of `{name, entry_level, boss_level, pdr_percent,
  party_max, force: {kind: arcane|sacred, value}, hp: [{phase, value}],
  recommended_spec: {kind, text}, notes}`. Only `name` is required. `name` is
  `Easy|Normal|Hard|Chaos|Extreme` and must be a catalog difficulty, or
  `Champion|Destiny`, which appear only on the Bosses info page and in chat
  guides (never in the scheduler, so not in the catalog). Up to 7 entries.
  `Destiny` is the Destiny Weapon mission (its modifier and Adversary's
  Resolve cost in `notes`); `Champion` is the Union Champion trial. `hp.phase`
  is `'1'`, `'2-1'`, or `total` when phases are HP thresholds on one bar.
  Unit-bearing values stay strings (`241.5t`, `10.266q`).
- `strategies` (added 2026-10): up to 4 named routes, each `{name, when, risk:
  low|medium|high, damage: low|medium|high, payoff, steps}` (1-6 steps).
  `damage` is the damage requirement. Only routes a source describes; general
  advice stays in `tips`.
- `event.aliases` (added 2026-10): other names members use (spellings, the
  Korean name), matched case-insensitively by the chatbot. Event bosses stay
  answerable until their file is removed or edited by hand.
- MapleSEA wording: SEA patch-note terms first, other names once as "also
  called". `force.kind: sacred` is MapleSEA's Authentic Force. Exception: Carling's
  Perils are Tiger (Do'oul), Bird (Gunggi) and Dog (Hondon), introduced once
  with the SEA name and the animal name afterwards (user decision 2026-10-05). HP rows are the
  KMS values before OVERDRIVE (what MapleSEA has now); a per-difficulty note
  gives the post-OVERDRIVE value until MapleSEA ships it.
- Guide redesign (added 2026-10, all optional):
  - Bullets (`core`, `danger`, `tips`, `notes`, `difficulty_notes` values,
    `difficulties[].notes`, `phases[].items`) are plain text or `{title (≤40),
    text (≤180), detail? (≤500)}`. The page shows title and text; only the
    chatbot reads `detail` (it gets `Title: detail`, else `Title: text`).
  - `lead` (≤160): the page lead. `summary` is still what the chatbot reads.
  - `phases`: `[{name, items}]`, the core mechanics by phase (1-6 phases, 1-5
    items). With `phases`, `core` may be omitted or hold only cross-phase items;
    a document needs at least one of them. The Phases tab draws them as one
    timeline: optional `tag` (≤40 caption) and `tone` per phase, and `group`
    (≤40) shared by adjacent phases, with `cycle: true` on every phase of a
    group that repeats (Seren's Phase 2 clock, alternating states).
  - `hp[].count` (2-6): copies at that HP (three Perils). `recommended_spec`
    may add `value` (short figure, `≈ 86k`) and `basis` (`KMS, Oct 2025`).
  - `difficulties[].mission`: `{series: destiny-weapon|union-champion, order
    (1-12), title, modifier?: {text, direction: up|down}, needs?, rules?}`;
    `up` is in the player's favour. The knowledge API lists every boss in the
    same series by `order`, so keep each series' orders distinct (`validate.py`
    fails a shared place).
  - `mechanics`: up to 4 diagram blocks with a `title` and optional `note`:
    `ledger` (`rows: [{label, value, direction?}]`; `direction` as for a
    mission modifier: `up` in the player's favour, `down` against), `zones` (`zones: [{name,
    sub?, tone?}]`, 2-4) or `scale` (`bands: [{label, span, tone?}]`, 2-7,
    `span` a relative width). `tone` names a colour role
    (`red|yellow|green|blue|neutral|risk|safe`); the chatbot reads only
    `risk`/`safe`.
- `sources` (1-14) entries are objects: `{url (https), title, author, kind:
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
