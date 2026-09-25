# v4 portal → v5 PWA parity

Status: batch 4 (Config, channel access, per-run re-read, lazy planner, limits + admin-API contracts), 2026-09-24. Tracks every
v4 page, partial and HTML route against its v5 screen or component, the JSON
API it needs (`docs/v5/inventory.json` `routes`), its DTO
(`web/packages/api-types`) and agreed v5 changes. Visual language:
`pwa-design-guidelines.md`; component map: `web/packages/tokens/README.md`.

Status: **done** = built and tested in v5; **partial** = built, with the gaps
noted; **todo** = not built yet;
**later** = public app, not started; **removed** = no v5 equivalent by design.

Summary: 38 rows in the page and partial tables — 35 done, 2 partial, 0 todo,
1 removed (batch 3: 30 done, 6 partial, 1 todo, 1 removed; batch 2: 20 done,
5 partial, 12 todo, 1 removed; batch 1: 36 rows — 9 done, 6 partial, 20 todo,
1 removed). Public app: 1 partial (the portal-off window is built), 3 later.
Every v4 section is built; the partials are
Limits overrides and login auth.

## Pages (`legacy/python/bot/api/templates/*.html`)

| v4 page | v5 route / component | API | DTO | v5 changes | Status |
|---|---|---|---|---|---|
| `base.html` (masthead, grouped nav, footnote) | `apps/admin` `App.svelte`, `shell/Nav.svelte`, ui `Masthead` | `GET /api/identity`, `GET /api/admin/session` (new) | `Identity`, `Session` | History-API routes; phone keeps Week + Inbox pinned, More holds the rest and Sign out; fixed `100dvh` frame at every width | done |
| `week.html` | `/` `pages/WeekPage.svelte` | `GET /api/schedule` → mock `GET /api/admin/week?week=`, `/stats`, `/summary`, `/members`, `/channels` | `Week`, `Stats`, `Summary`, `Member`, `Channel` | v4's header row carries This/Next week, the Planner / Runs / Answers switch and a "How to move runs" disclosure; the board sits straight under the filter card (no window chrome); tiles and filters degrade under the area budget (one-line tiles → summary line; "Filters (n)" with chips on phones and short frames); drag + keyboard moves with undo; done/cancelled runs hidden behind "show them" | done |
| `fixed.html` | `/fixed` `fixed/FixedPage.svelte`, `FixedEditor.svelte` | `GET/POST /api/fixed`, `PATCH/DELETE /api/fixed/{id}`, `POST /api/validate/bosses` (mock `/api/admin/fixed*`, `/api/admin/validate/bosses`) | `FixedRow`, `FixedRequest`, `ValidateResult` | Update-or-keep choice per amended run on edit; Remove → Retire with a named-consequence confirm; Reset to fixed on runs | done |
| `bosses.html` | `/bosses` `bosses/BossesPage.svelte` + `BossGrid` | boss catalog read route (new; mock `GET /api/admin/bosses`) | `BossRow`, `DifficultyOption` | — | done |
| `boss_knowledge.html` | `/bosses/:boss/knowledge` `bosses/KnowledgePage.svelte`; event bosses listed on `/bosses` | `GET /api/bosses/{boss}/knowledge` (mock serves tracked `boss/knowledge/*.yaml`), events read route (new) | `Knowledge`, `KnowledgeDoc`, `DifficultyFacts`, `KnowledgeSource`, `EventBoss` | Schema v2: summary, core/danger/tips, per-difficulty facts behind a difficulty switch (opens on `?difficulty=`, else the guild's difficulty), difficulty notes, sources credited with author/kind/fetched/updated, Event badge and availability (Kai) | done |
| `inbox.html` | `/inbox` `inbox/InboxPage.svelte`; nav pip + Inbox tile use `Summary.inbox` | `GET /api/pending`, `POST /api/amendments/{id}/approve` (optional edit), `POST …/reject` (mock `/api/admin/inbox*`) | `Proposal`, `Evidence` | One window with Extractor / Self-service title-bar tabs (counts), a listbox of boss + type, who, when and badges (conflict, expired, frozen requester, already in effect) beside the detail (evidence quote, preview + conflicts, per-run choices for weekly timings, Approve / Move & approve / Reject with a reason); `?tab=&item=` deep links; phones: list, then detail with Back; approvals attributed in History (`extraction_approval` / `admin_portal`) | done |
| `extractions.html` | `/extractions` `extractions/ExtractionsPage.svelte` + `RescanPanel` | `GET /api/extractions`, `GET /api/rescan/targets`, `POST /api/rescan`, `GET/DELETE /api/rescan/{id}` | `Extractions`, `ExtractionRow`, `RescanJob` | Paged; server-side filters (model, dates with guild-time presets, outcome, channel, member, text) deep-linked in the query string, chips + Clear; the rescan job is polled with the bounded poller (progress per channel, cancel) | done |
| `extraction.html` | `/extractions/:id` `extractions/ExtractionPage.svelte` | `GET /api/extractions/{id}` | `Extraction` | Tabs: Changes, Chat read, Prompt as sent, Raw response | done |
| `chat.html` | `/chat` `chat/ChatPage.svelte` | `GET /api/chat`, `GET /api/chat/summary` (mock: one response) | `Chat`, `ChatSummary`, `ChatRow` | Per-model statline over the filtered rows; server-side filters (as Extractions, plus tool used and minimum latency); paged | done |
| `chat_interaction.html` | `/chat/:id` `chat/ChatTurnPage.svelte` | `GET /api/chat/{id}` | `ChatTurn` | Tabs: Conversation, Tool trace, Model trace, Produced, Raw content | done |
| `limits.html` | `/limits` `limits/LimitsPage.svelte`; Model tile uses `Summary.model` | `GET /api/admin/limits` (v5 shape), `DELETE /api/admin/limits/windows/{id}`; overrides later | `Limits`, `BackendGroup`, `Refusal`, `Allowance` | Backend groups (permits, queue with positions, rate bucket, retry budget, breaker), admission refusals by kind incl. key-level, allowances with reset; polled | partial: override editing and the member-facing view are later; the DTO is a proposal (`docs/v5/limits-contract.md`) |
| `members.html` | `/members` `members/MembersPage.svelte` | `GET /api/members`, `PATCH /api/members/{id}`, `POST /api/members/{id}/nick` (mock `/aliases`), personas read route (new) | `MemberRow`, `MemberPatch`, `Persona` | Ping level and reply style editable in the sheet; bossing role and chatbot access shown | done |
| `reminders.html` | `/reminders` `reminders/RemindersPage.svelte` | `GET /api/reminders` | `Reminders`, `ReminderRow` | States queued / due / sent / stale; `?run=` narrows to one run (linked from the sheet's Cards); sent list paged | done |
| `config.html` | `/config?section=` `config/ConfigPage.svelte` + per-section components | `GET/PATCH /api/admin/config` (one section per save), `POST /api/admin/config/profiles/reload`, `POST /api/admin/digest` (with channel override), `GET /api/admin/access`, `POST /api/admin/access/recheck` | `ConfigView`, `AccessReport`, `ReplyProfile` | One settings window, twelve sections: pings, watching, chatbot rates (+unconfigured state naming the missing env), persona catalog with read-only file profiles (publish/unpublish, reload, ordered role assignments with first-match-wins), three model roles with published-effort reasoning incl. model-decides levels and fail-closed cloud/unknown-trust/unlisted-alias warnings, capacity groups validated against per-alias admission and the shared key cap, self-service redirect modes + public portal switch, notifications, theme, digest, re-read, Channel access section, env-only table with reasons (incl. watched categories), Manage-Messages banner above all sections | done (profile text editing/creation/deletion is files-only by decision — a deliberate v4 drop; role/behaviour plugin routes are replaced by publish + ordered assignments) |
| `audit.html` | `/history` (`/audit` redirects) `history/HistoryPage.svelte`, `RevertDialog`; run-sheet `sheet/BlamePanel.svelte` | `docs/v5/history.md` operations; mock `GET /api/admin/history` (week/actor/before paging), `GET …/{seq}`, `POST …/revert`, `…/restore-week`, `…/revert-actor` (preview/force/request_id), `GET …/checkpoints`, `GET /api/admin/runs/{id}/blame` | `ChangeRecord`, `HistoryPage`, `RevertPlan`, `RollbackMode`, `Checkpoints`, `BlameEntry` | Per-week timeline with actor, surface, `reverts #n`, row diffs; revert / restore week to a point / revert a member, each previewed with a conflict report and an acknowledged Force; checkpoints with chain verification; blame per run field | done (mock shapes; real API pending) |
| `login.html` | `/login` `pages/LoginPage.svelte` | `GET /api/identity`; auth routes TBD | `Identity` | Discord OAuth primary, Tailscale identity fallback, admin token as break-glass (copy and flow only); banner/avatar with generated stand-ins | partial: auth not wired |
| `error.html` | unknown path → `pages/NotFoundPage.svelte`; API errors → toasts / inline | — | `ApiError` | — | done |

## Partials (`templates/partials/*.html`)

| v4 partial | v5 component | Status | Notes |
|---|---|---|---|
| `now.html` | `week/NowTiles.svelte` | done | Next (opens the run sheet), Unanswered, Inbox → `/inbox`, Model → `/limits`; phone: one scrolling strip |
| `rail.html` | none | removed | Phone week shape: v5 stacks the board's days inside the one scrolling panel instead |
| `days.html` | `planner/Planner.svelte` narrow layout | done | Day list on phones = stacked board columns; each card carries its channel's re-read button (phones), one channel over this boss week |
| `board.html` | ui `DayColumn` + `RunCardBody`; admin `PlannerColumn`/`PlannerCard`; public `Board` | done | Quiet days collapse to spines; lead boss entry art under the veil; tokens not names only |
| `run_sheet.html` | `RunSheet.svelte` (ui `Modal` wide + flush) | done | Results and Undo show inside the sheet (a modal makes toasts inert) |
| `run.html` | `RunSheet.svelte` article `.run` | done | Split entry art, portraits + level, status/tally/out/maybe/waiting, channel with its re-read button (reported in the sheet), short id, chips with remove and add, "this week: −A +B" line, cards timeline, Move (`parseWhen`) + Preview ping + Reset to fixed, status control, answers editor |
| `macros.html` `portrait`/`boss_line`/`boss_list` | ui `Portrait`, `BossTag` | done | Monogram hue via CSSOM custom property, never a style attribute |
| `macros.html` `boss_grid`, `search_form`, `no_match` | `bosses/BossGrid.svelte`, `pages/PaneWindow.svelte` | done | Search filters as you type |
| `macros.html` `pager`, limit actions | `pages/Pager.svelte` + `paging.ts`; Limits reset button | done | Members, Reminders, Extractions, Chat paged client-side; History pages by API cursor |
| `bosses.html`, `bosscheck.html` | `FixedEditor` "…or type them" status | done | Debounced `POST /api/validate/bosses`; unknown boss or difficulty named in words |
| `fixed_rows.html` | `FixedPage` table + `FixedEditor` | done | |
| `member_rows.html`, `reminder_rows.html` | `MembersPage`, `RemindersPage` | done | |
| `audit_rows.html`, `chat_rows.html`, `extraction_rows.html` | History timeline, `ChatPage`, `ExtractionsPage` | done | |
| `member_sheet.html` | `members/MemberSheet.svelte` | done | |
| `limits.html` | `LimitsPage` tabs | done | Polled every 5 s with the bounded poller |
| `rescan_job.html` | `extractions/RescanPanel.svelte` | done | Bounded poller instead of `hx-trigger`; progress in a polite live region |
| `access.html` | `config/AccessSection.svelte` (Config → Channel access) | done | The bot role's permissions per channel, digest/not-watched chips, the Manage-messages exception in words, Check again |
| `flash.html` | ui `ToastRegion` + inline `field__error`/sheet notice | done | |
| `icons.html` | ui `Icon.svelte` (same Feather paths) | done | |
| `theme_boot.html` | `@kanade/ui/theme-boot.js` (external, hashed) | done | Same storage keys |
| cards in `run.html` | `RunSheet` `.run__cards` | done | Timeline per run; "Cards" links to that run's reminders |

## Week widgets (reference `portal-week.png`, `portal-run-sheet.png`)

| Widget | v5 | Status |
|---|---|---|
| Branded masthead (avatar, name, tz, powered by kanade, caller, sign out) | `Masthead` + identity | done |
| Grouped nav SCHEDULE / KANADE / OPERATE with Inbox pip | `Nav.svelte` | done |
| This week / Next week | `?week=next` route, `seg` links | done |
| Now tiles | `NowTiles` | done |
| Filter bar channel / member / boss | `Filters.svelte`, live, folds on phones | done |
| Quiet days as spines | `.board__col--empty` | done |
| Run cards with entry art under the veil | `RunCardBody` `<img class="runcard__art">`, `--art-veil`, `--art-crop-card` | done |
| Run sheet (portraits, level, artwork background, chips with answers, cards timeline, status control, Move + Preview ping, answers editor) | `RunSheet.svelte` | done (see `run.html` gaps) |
| Past / cancelled runs hidden by default | `WeekPage` "N past or cancelled runs hidden · Show them" | done |

## HTML routes (`inventory.json`, policy `portal`/`portal-auth`/`portal-sse`, all Remove)

| v4 route | v5 |
|---|---|
| `GET /` | `/` Week |
| `GET /fixed`, `POST /fixed/new`, `POST /fixed/{id}/edit`, `POST /fixed/{id}/delete`, `POST /validate/bosses` | `/fixed` (done) over `/api/fixed*` |
| `GET /inbox`, `POST /inbox/{id}/approve`, `POST /inbox/{id}/reject` | `/inbox` (partial) over `/api/amendments/*` |
| `GET /extractions`, `GET /extractions/{id}` | `/extractions`, `/extractions/:id` (done) |
| `GET /chat`, `GET /chat/{id}` | `/chat`, `/chat/:id` (done) |
| `GET /limits`, `GET /limits/live`, `GET /limits/events` (SSE), `POST /limits/windows/{id}/reset`, `POST /limits/overrides`, `POST /limits/overrides/{id}/clear` | `/limits` (partial: overrides later); SSE replaced by the bounded poller |
| `GET /audit` | `/history` (done; `/audit` redirects) |
| `GET /access`, `POST /access` | `/config?section=access` (done) over `GET /api/admin/access`, `POST /api/admin/access/recheck` |
| `GET /bosses`, `GET /bosses/{boss}/knowledge` | `/bosses`, `/bosses/:boss/knowledge` (done) |
| `GET /members`, `POST /members/{id}/nick` | `/members` (done) |
| `GET /reminders` | `/reminders` (done) |
| `GET /config`, `POST /config`, `POST /digest` | `/config` (done) over `GET/PATCH /api/admin/config`, `POST /api/admin/digest`; role/behaviour plugin editors are replaced by the persona catalog + per-role profiles |
| `POST /rescan`, `GET /rescan/{id}`, `POST /rescan/{id}/cancel` | Extractions → Re-read (done) |
| `POST /runs/{id}/amend`, `/cancel`, `/otot`, `/status`, `/restore`, `/participants`, `/rsvp`, `/ping` | Run sheet + planner over `/api/runs/{id}/*` (mock: `/api/admin/runs/{id}/{move,status,rsvp,participants,ping,reset}`; `reset` is new) — done |
| `GET /login`, `POST /login`, `GET /logout` | `/login` window (partial; auth design pending) |
| `GET /static/portal.css` | removed: Vite-built hashed assets |
| `GET /static/portraits/{short}`, `GET /static/entry/{short}` (Retain) | `/art/portraits/{key}`, `/art/icons/{key}`, `/art/entry/{key}`; null URL when absent |
| `GET /identity/avatar`, `GET /identity/banner` (Retain) | same paths; generated SVG when nothing is cached |

## Public app (later)

| Page | Status |
|---|---|
| Week (read-only board + list) | partial: built; art shown |
| My runs | later |
| Requests (member self-service) | later |
| Limits (my allowance) | later |

## v4 live alignment (batch 5)

Compared every admin screen with read-only captures of the live v4 portal
(`web/e2e/.captures/v4-live/`, private: real guild data, layout reference
only, never copied) against v5 real-art captures at the same sizes
(`KANADE_REAL_ART=1 bunx playwright test capture -g comparison` →
`web/e2e/.captures/real/compare/v5-<page>-{wide,narrow}.png`; wide 1280×800,
narrow 422 px like v4's).

| Area | Outcome | What changed or why not |
|---|---|---|
| Page heads on laptop-height frames | aligned | v4's short-screen rule ported (≥900 px wide, ≤850 px tall): eyebrow and explanatory note hide, head padding tightens, footnote hides. Week reads "N runs · This/Next week" in one row like v4. |
| Config page | aligned | No page head; the window is titled "Config" in its own bar with v4's one-line subtitle. |
| Fixed table | aligned | When leads (v4 column order); bosses stay the row header. |
| Stat cards (Limits) | aligned | v4's 240 px minimum: one readable column on phones. |
| Phone week shape | aligned | v4's seven-cell rail is back (admin and public), sticky at the top of the scrolling panel; a cell jumps its day into view inside the panel. |
| Planner help | aligned | A "How to move runs" disclosure button in the week header at every width (parent decision); every movable card keeps `aria-describedby` on it. |
| Type sizes | aligned | Component-local sizes mapped to type-scale tokens (+ `--fs-clock`, `--fs-clock-narrow`, `--fs-avatar`). |
| Masthead, grouped nav, window chrome, title-bar tabs, tables, forms, buttons, boss grid | already matching | Same tokens and partials; spacing within a few px. |
| Week board | aligned (batch 6) | The board sits straight under the filter card with no window chrome, as in v4; Planner / Runs / Answers moved into the week header as a second segmented control. 1280×800: board 456 px (v4 ≈422); 1000×670: 414 px (v4 ≈292). |
| Week "Show them" note on laptops | kept | v4 hid it with the other prose; v5 keeps a note that carries a control. |
| Filter button, Fixed/roster Search buttons | kept (v5) | Filters and searches apply live. |
| Run sheet | kept (parent decision) | A modal sheet (a bottom sheet on phones), not v4's in-page sheet. |
| Phone tab strips (Config sections, window tabs) | kept (agreed frame) | v4 wrapped them into rows and let the body scroll; the fixed frame makes them scroll sideways inside the strip. |
| Phone Now tiles | kept (agreed frame) | v4 stacked four full-width tiles in a scrolling body; v5 keeps one sideways strip so the window keeps height. |
| Masthead extras (Live freshness, Commands) | kept (v5 features) | Not in v4. |
| Memory nav item, Audit | removed / replaced | Memory is removed; Audit is History. |

## Captures and comparison

Placeholder-art captures (CI-safe): `cd web && bunx playwright test capture` →
`web/e2e/.captures/synthetic/` (placeholder art, not the game's). Real local art for review:
`KANADE_REAL_ART=1 bunx playwright test capture` → `web/e2e/.captures/real/`
(both git-ignored). Each set: `admin-{week,sheet,fixed,fixed-editor,fixed-choice,bosses,knowledge,knowledge-event,members,member-sheet,reminders,inbox,extractions,extraction,chat,chat-turn,limits,history,history-revert,config,login}-` and
`admin-config-access-`, `admin-config-models-`, `admin-config-models-cloud-`, `admin-config-persona-`, `admin-config-self-service-` and
`public-week-` × `{wide,narrow}` × `{marigold-light,marigold-dark,twilight-dark}`,
plus the v4 comparison set `compare/v5-<page>-{wide,narrow}`, `admin-wide-lifted`, `admin-wide-palette`, `admin-week-grip-hover-wide-*` and
`admin-week-grip-focus-wide-*`.

Compared with `legacy/python/docs/images/`:

- `portal-week.png` ↔ `real/admin-week-wide-marigold-light.png`: same masthead
  (avatar, name, meta row, three labelled nav groups, Inbox pip), page head with
  the week toggle, four tiles, filter bar, board with quiet-day spines and art
  veils. Differences: v5 puts the board in a tabbed window (Planner/Runs/Answers)
  with a small grip in each card's corner (the whole card drags; `M` moves it
  from the keyboard) instead of v4's move affordance; the filter bar has no Filter button (it filters live).
- `portal-run-sheet.png` ↔ `real/admin-sheet-wide-marigold-light.png`: same
  layout — clock and day, portraits with pills and levels, the tally line,
  answer chips with × and "+ add…", the Cards timeline, Move + Preview ping over
  the artwork, the status segmented control and the answers disclosure. The
  title bar carries the window dots; duplicate names gain `#member-id`.
- `portal-login.png` ↔ `real/admin-login-wide-marigold-light.png`: same gate
  window; with nothing cached the banner/avatar are the generated stand-ins.
- Batch 4: `/config` is complete — `admin-config-*.png` shows the settings window (Pings) and `admin-config-access-*.png` the access table.
- Batch 3 (`real/admin-{inbox,extractions,extraction,chat,chat-turn,limits,history,history-revert,knowledge,knowledge-event}-*.png`):
  Inbox keeps v4's card per proposal with evidence, confidence and Approve /
  Move & approve / Reject; Extractions and Chat keep v4's pane tables and
  tabbed details; Limits and History are new v5 designs in the same window
  language (stat tiles, timeline). Placeholder-art captures now go to
  `web/e2e/.captures/synthetic/`.
- Batch 2 (`real/admin-{fixed,fixed-editor,fixed-choice,bosses,knowledge,members,member-sheet,reminders}-wide-marigold-light.png`,
  plus `-narrow-` and `-twilight-dark` variants): Fixed follows v4's table and
  editor window (boss grid with pill toggles, typed bosscheck, day/time/channel/
  note, party chips); the update-or-keep step is new. Bosses, Members and
  Reminders follow v4's pane windows; the member sheet adds ping level and
  reply-style controls.
