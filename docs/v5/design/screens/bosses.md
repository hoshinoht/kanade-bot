# Bosses and boss knowledge

**Purpose:** the boss catalog and each boss's checked-in knowledge
(`boss/knowledge/*.yaml`, schema v2), including seasonal (event) bosses.

**Board:** `B_Bosses` (pair: bosses, 1280×800 and 390×844, fidelity-bosses).

## As built

Code: `apps/admin/src/bosses/` (`BossesPage`, `BossWorkspace`, `BossGrid`,
`KnowledgePage`, `StrategyList`, `heroArt.ts`, `event.ts`);
`_boss-knowledge.scss`, `_boss-grid.scss`, `_bosses.scss`.

- List pane 330 px (`.bosses-list`) beside the knowledge detail;
  `/bosses/:key/knowledge` deep link; below 900 px (the shared single-pane
  switch) one at a time, as Chat: a pick pushes a tagged entry and focuses
  the detail; the "← Back to the catalog" button (rail frame) or the top
  bar's back (phone frame) pops it, or replaces a deep link with `/bosses`,
  and focus returns to the boss's link. `[DR 2026-10-05]`
- Knowledge header is one aligned line: title · Lv · researched date · source
  `[DR 2026-10-02]`; aside 240 px with weekly timings using the boss.
- Seasonal bosses carry a tonal chip "Seasonal boss · Challengers World
  Season N" in the list and the header `[DR 2026-10-02]`.
- Knowledge hero plays a muted looping MP4 over the still poster; still image
  under reduced motion or on error; service worker bypasses `/art/animated/`
  (`BossWorkspace.svelte:152`) `[DR 2026-10-03]`. Fidelity captures use
  stills only.
- Values come only from the YAML; board `[FROM YAML]` markers are not data.

## Known gaps / Planned

- **Planned:** port this screen to the public portal (signed-in members only,
  read-only guide + "your runs with this boss this week"); see
  `../../public-portal-plan.md`. `[DR 2026-10-04]`
- No animated art in Discord cards. `[DR 2026-10-04]`
