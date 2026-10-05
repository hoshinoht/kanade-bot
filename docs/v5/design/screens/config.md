# Config

**Purpose:** server settings, one section at a time. Budget: the open
section's panel > the contents list. `[guide]`

**Boards:** `B_Config` (Chatbot), `B_CfgPings`, `B_CfgWatch`, `B_CfgPersona`,
`B_CfgRoles`, `B_CfgModels`, `B_CfgSelf`, `B_CfgNotify`, `B_CfgTheme`,
`B_CfgDigest`, `B_CfgReread`, `B_CfgAccess`, `B_CfgEnv`. All 13 pairs approved
2026-10-02; pairs in `fidelity.spec.ts`: cfg, cfg-persona, cfg-models,
cfg-roles.

## As built

Code: `apps/admin/src/config/` (`ConfigPage`, one `*Section.svelte` each,
`SettingsPanel`, `SaveBar`, `SwitchCard`, `PillTabs`, `ThemeTiles`,
`TokenSlider`, `ContextWindows`, `find.ts`, `save.ts`, `dirty.ts`,
`capacity.ts`); `_settings.scss` (imported as `settings.scss`).

- Page line h1 "Config"; the window is titled **Settings** with a "find a
  setting…" search (filters the list and jumps to and flashes the matched
  card; no flash under reduced motion). `[DR 2026-10-01]`, `[DR 2026-10-03]`
- Problem alert: a risk chip in the page line, "Manage Messages missing in n
  channels · fix in Channel access", linking to that section (replaces the
  old flash banner). `ConfigPage.svelte:333`
- Contents list 230 px (`14.375rem`), a vertical `tablist` deep-linking
  `?section=`; a sideways strip below 900 px. Sections (`ConfigPage.svelte:50`):
  **Bot** Pings, Run lengths, Chat watching, Chatbot, Persona, Models ·
  **Members** Self-service, Notifications, Weekly digest · **Server**
  Re-read, Channel access, Theme · **Read-only** Set in the environment.
  Value hints stand out (not a font bug) `[DR 2026-10-02]`.
- Visited sections stay mounted so drafts survive switching; each section saves
  itself with a partial `PATCH /api/admin/config`, refusals inline, 409 keeps
  the draft and offers Reload. `[web/AGENTS]`
- **When changes apply:** switches apply at once (confirm before turning off
  or opening the portal; toast with Undo); form fields save with the sticky
  save bar ("n unsaved change · field old → new", Discard, Save <Section>);
  actions run from their own buttons. `[old §Config]`
- Long sections split into in-panel pill tabs (Persona: active + profiles /
  role overrides; Models) `[DR 2026-10-02]`; pill focus rings sit inside the
  pill (`outline-offset: -2px`) `[DR 2026-10-03]`.
- Section specifics as built `[DR 2026-10-03]`: Pings countdown chips with ×
  and an add field; Re-read checkbox chips and running-job card with
  `WavyProgress`; Theme swatch tiles grouped by collapsible set; Models
  "Startup check" column (server checks only), capacity bars that wave while
  calls run (`in_use`); Persona "Default voice" is a UI-only first row;
  Digest "Last posted" card ("week of <date>" for another week); Env copy
  buttons; Context windows "In effect now" has its own card heading.

## Phone

Contents list becomes a sideways strip; panel scrolls alone; the save bar
stays put.

## Known gaps / Planned

- **Planned:** a **Profanity** section (extra blocked words, built-in words
  allowed again, question/reply switches, deflection line; applied live).
  `[DR 2026-10-05]`
- **Planned:** "Quiet mode on" page-line chip (`B_States`, `B_CfgNotify`).
