# Shell and components

As built in `web/apps/admin/src/shell/`, `web/packages/ui/src/components/`
and `web/packages/ui/src/styles/`. Board names refer to
`docs/research/2026-09-28-m3e-mockups/` (and `2026-10-04-picker-mockups/` for
`P_*`). Citation keys: `README.md`.

## Frame and breakpoints

```
┌──────┬────────────────────────────────────────────────────────┐
│ rail │ page line (36 px, on the ground)                       │
│ 96 / ├────────────────────────────────────────────────────────┤
│ 240  │ window: 2 px --win border, --r-win 20, --shadow        │
│      │ ┌ title bar 48: dots · tabs/title · spacer · actions ┐ │
│      │ │ list pane │ detail / board │ side pane            │ │
│      │ └ optional footer ───────────────────────────────────┘ │
└──────┴────────────────────────────────────────────────────────┘
```

- `.frame`: `position: relative; height: 100dvh; overflow: clip`. `clip`, not
  `hidden`: a hidden box is still a scroll container that `focus()` and
  `scrollIntoView` can shift. `_frame.scss`, `[lesson]`
- The 1180 px `.shell` cap is lifted in the admin rail frame; panes set their
  own measures. The public app keeps the cap and `Masthead`. `[DR 2026-10-01]` (G6)
- Exactly one of rail or top bar renders, so `.fresh`, `.account__name` and
  the `Sections` nav stay unique. `[web/AGENTS]`

| Width / height | Navigation | Panes |
|---|---|---|
| ≥ 1440 wide | expanded rail 240 (unless collapsed; `localStorage` `rail`) | list + detail + side pane |
| 1200–1439 | rail 96 | list + detail; Week keeps its Glance pane |
| 900–1199 | rail 96 | two panes; Week Glance folds into the footer below 1200 |
| 600–899 | rail 96 | one pane; detail replaces the list with Back (840–899 still two panes on some screens until the Planned fix below) |
| < 600 wide **or** ≤ 500 tall | phone frame: top bar + drawer | one pane |

**Single-pane switch: 900 px for every list-detail screen** (user decision
2026-10-05). As built it still differs: Inbox, Chat, Extractions and Config
switch below 900 px (`max-width: 899px`); Members, Fixed, History, Bosses and
the Week run pane below 840 px. **Planned (queued):** one shared breakpoint
constant (next to `PHONE_QUERY` in `packages/ui/src/media.ts`) used by every
list-detail screen, moving the 840 px ones to 900 px. The dropdown's native
picker query (`select.ts` `NATIVE_QUERY`, 840 px) is not a pane breakpoint.
The phone frame applies below 600 px wide **or** at ≤ 500 px tall (phone
landscape; `[DR 2026-10-01]`; the old spec's "< 600 px" is stale).
`PHONE_QUERY = '(max-width: 599px), (max-height: 500px)'` in
`packages/ui/src/media.ts`. Rail compacts to 44 px items at ≤ 760 px tall.
`_nav-rail.scss`, `[DR 2026-10-01]`

## Navigation rail (G1, approved 2026-10-01)

`shell/Rail.svelte`, `shell/NavList.svelte`, `_nav-rail.scss`; destinations in
`apps/admin/src/routes.ts`.

- Collapsed 96 px: brand tile, items 80 px wide with a 56×30 active
  indicator (`--select` fill, `--select-ink`), label always visible, Inbox
  badge (`--accent-fill`, hidden at 0), group dividers, account avatar at the
  foot. Whole item is the link (≥ 44 px target). `[old §Navigation rail]`
- Expanded 240 px: brand + name + collapse button, group headings
  Schedule / Kanade / Operate, 40 px items with trailing counts.
- Order: Week, Fixed, Bosses │ Inbox, Extractions, Chat, Limits │ Members,
  Reminders, Config, History.
- A `nav` landmark of links; visible focus ring.

## Phone frame (G7, approved 2026-10-01)

`shell/TopBar.svelte`, `NavDrawer.svelte`, `EdgeSwipe.svelte`, `_topbar.scss`.

- **Top bar** 48 px + safe-area: menu, page title, Live chip, Inbox icon with
  badge. Detail views put "‹ Back" (e.g. "‹ Inbox") in the top bar through
  `chrome.back()` (`shell/chrome.ts`). **No bottom nav bar, ever.** `[DR 2026-10-01]`
- **Drawer:** `min(300px, 86vw)`, a modal `<dialog>` that traps Tab, opened by
  the menu button or a left-edge swipe (a vertical stroke does not), closed by
  scrim, Escape, × or navigation; focus returns to the menu unless a link
  changed the page. Shows Week, Members and Reminders counts; foot holds the
  account, time zone and the Ctrl/Cmd-K hint. `e2e/shell.spec.ts`
- Page line on phones has no status chip and no Commands pill; the palette is
  keyboard-only. `[web/AGENTS]`
- **Bottom action bar** on phone detail views (Inbox): radius `20px 20px 0 0`,
  safe-area padding; secondary icon, Reject…, key action filling the rest.
  `[old §Phone]`, `inbox/InboxDetail.svelte`

## Page line (G2, approved 2026-10-01)

`shell/PageLine.svelte`, `_page-line.scss`. Every admin page uses it instead
of a page-head card.

- 36 px tall; controls 32 px; unboxed on the ground. Children: the title group
  (breadcrumb / h1 / short context) in `--ground-ink`; a `side` snippet holds
  the page's own controls, each its own chip, button or field.
  `[DR 2026-10-02]` (fidelity audit)
- Sections: `title` set; the h1 is then the count with numerals in
  `.pageline__num` (bold mono) and context after " · ". Detail pages: no
  title, the h1 is the title. No ⓘ in the page line. `[web/AGENTS]`
- Live chip 28 px "Live HH:MM" (its `title` names the guild time zone, printed
  from 1440 px) + "Ctrl K" Commands pill. Page-line context colour stays
  `--ground-ink` (board's `--dim-text` not adopted). `[DR 2026-10-02]`
- The Live chip is `components/Freshness.svelte`: Loading… / Live · updated
  HH:MM / Retrying · last updated / Offline · last updated / Closed / "Can't
  reach Kanade"; icon plus words, never colour alone.
- **Planned:** a "Quiet mode on" page-line chip while quiet mode is on
  `[old §Config Notifications]`; not built.

## Windows and title bars

`_m3e-primitives.scss`, `_card-window.scss`, `_base.scss`, `_tabs.scss`.

- A page window is a `.card` with `.card__head` as its 48 px title bar
  (`--win` fill, `--win-ink` text), panes flush to the frame (no inset box).
- **Window dots:** `.card__head::before` / `.modal__head::before` draw the
  three 8 px dots (12 px apart, 55 % opacity) once. Never add dots in a screen.
  In tabbed bars the dots sit on the tabs' centre line (15 px above the floor).
  `[lesson]`, `[DR 2026-10-02]`
- **Title-bar tabs:** 38 px, `16px 16px 0 0` corners on the bar's floor;
  active tab `--surface` with `--ink`; count badge 18 px ring (selected:
  `--select`). Proper `tablist`/`tab`/`tabpanel` with arrow keys.
- **Title-bar controls:** outlined 30–32 px pills (1.5 px, 45 % `--win-ink`);
  icon buttons square with `aria-label`; any `[role="search"]` holding a search
  input becomes a 32 px surface pill with a CSS-drawn magnifier.
- **Footer** (optional, Week, Reminders): one line of facts, counts and
  removable filter chips.
- **No window in a window** (`[DR 2026-10-05]`): a modal sheet is one window;
  its identity card and tabs are sections, not a nested `.card`. Known
  violation: the full run sheet (`screens/run-sheet.md`).

## Panes

| Piece | Code | Rules |
|---|---|---|
| List pane | `components/ListPane.svelte`, `.list-pane` | `--pane` fill, scrolls alone, `role="listbox"` with `aria-activedescendant`, arrow keys + Enter; options are `--row` cards radius 6, 3 px apart, padding 12 |
| Selected row | per screen (`*--active`) | `--select` fill, `--select-edge`, radius 16–18, title 700; "open" label screen-reader only |
| Grouped rows | e.g. Glance, Config | first `16 16 4 4`, middle 4, last `4 4 16 16` |
| Detail pane | per screen | `.cap` eyebrow, `--display` 800 heading, meta line; key action at the header end; pill tabs (30 px, radius 15, `--select` when active) are a `tablist` |
| Side pane | `components/SidePane.svelte`, `.side-pane` | `aside` with a label, border-left 1.5 px `--line`, `--surface`, scrolls alone; widths: default `min(380px, 38vw)` (Members), `--history` 400, `--fixed` 420, Week run pane `clamp(380px, 32vw, 480px)` (340 at 840–999) |
| Thread panel | `components/ThreadPanel.svelte`, `.thread-panel` | radius 20, `--row`, inset `--line` ring; message rows with flower avatars; "used" rows `--select` + label |
| Decision card | `components/DecisionCard.svelte`, `.decision-card` | radius 28, `--select`, padding 18; key Approve; Reject… apart at the foot |
| Code viewer | `extractions/CodeViewer.svelte` | `--mono` `--fs-mini`, line numbers, Wrap, Copy, find |

- Opening a pane: the list stays still; Escape closes; focus returns to the
  row (`preventScroll`). A fixed window uses `overflow: clip` so programmatic
  focus cannot scroll it. `[lesson]`
- Pane switch below the cut-off: detail replaces the list; a pick pushes a
  tagged history entry so Back returns to the row. `[web/AGENTS]`

## Buttons, fields, groups, chips

`_m3e-buttons.scss`, `_m3e-primitives.scss`, `_forms.scss`, `_chips-status.scss`.

| Element | Class | Size / shape | Fill |
|---|---|---|---|
| Secondary | `.btn` | 32 px pill, 1.5 px border | `--surface` |
| Key | `.btn--primary` (`.btn--key` = 44 px, 700) | pill | `--accent-fill` / `--accent-ink` |
| Risk key | `.btn--risk` | pill | `--risk-text` fill, `--surface` text |
| Destructive | `.btn--danger` | pill outline | risk-wash border, `--risk-text` |
| Connected group | `.seg` | 32 px, segments 2 px apart, outer 16 / inner 6, selected full pill | `--seg-fill`; selected `--accent-fill` |
| Status chip | `.status-chip--{neutral,ok,risk,warn}`, `.tone` | 22 px pill, `--fs-mini` 600 | tonal; warn is an outline |
| Overline | `.cap` | see `foundations.md` | — |
| Switch | `role="switch"` (`config/SwitchCard.svelte`) | 32×52 | on `--accent-fill` |

- Changed (unsaved) field: 2 px `--accent-fill` ring. `[old §Buttons]`
- Disabled-but-explained actions (e.g. blocked Approve) stay focusable with
  `aria-disabled` and a one-line reason. `[old §Inbox: Self-service]`

## Pickers (built 2026-10-04, boards `P_*`)

| Picker | Code | Behaviour |
|---|---|---|
| Dropdown | `components/Select.svelte`, `MultiSelect.svelte`, `SelectList.svelte`, `select.ts`, `dropdown.ts`, `_select.scss` | select-only combobox, focus stays on the pill; sizes `bar` 36 (filter row), `field` 40 (forms), `tbar` 32 (title bar); search above 10 options; groups; popover placed by CSSOM custom properties. Below 840 px or on a coarse pointer the pill wraps a transparent native `<select>`; MultiSelect opens a full-screen sheet. Replaces every native select (36). `[DR 2026-10-04]` |
| Date / range | `components/DatePicker.svelte`, `MonthGrid.svelte`, `calendar.ts`, `_date-picker.scss` | Thursday-first (reset day) month grid, `range` (Apply) or `single` ("Since"), quick chips, typed-field errors, phone bottom sheet; today is the server's |
| Move (day + time) | `sheet/MovePicker.svelte`, `components/DayStrip.svelte`, `TimeStepper.svelte`, `sheet/parseWhen.ts`, `_move-picker.scss` | day strip (selected pill, today ring, run's own day dashed, past days struck/disabled, up to three run dots), time spinbutton stepping by Run lengths (PgUp/PgDn ±1 h, wraps), typed shortcut ("wed 21:30") parsed live, suggestions from the picked day with clash wording |

- Filter dropdowns live in the Filters (n) popover; only Dates sits in the
  filter row. `[DR 2026-10-04]`
- **Planned:** the Fixed editor's Time field uses `TimeStepper` beside its
  weekday strip (typed entry kept). `[DR 2026-10-05]`

## Modals and sheets

`components/Modal.svelte`, `_modal.scss`.

- Native `<dialog>` with `showModal()`, title bar with dots, Escape closes,
  focus returns to the trigger; `lightDismiss` closes on a backdrop press only
  when nothing is unsaved. `[DR 2026-10-04]`
- Confirm dialogs name the object and consequence; destructive ones start
  focus on Cancel and use the risk key. Used for Retire…, Reject…, Revert…,
  turning a switch off, opening the public portal, Post it now…, Use this
  persona. `[old §Shared states]`
- One window per modal (MUST 7). Run sheet: `screens/run-sheet.md`.

## States

| State | Code | Rules |
|---|---|---|
| Empty | `components/StateNote.svelte` | cookie glyph 112 px on `--select`, heading `--display` 800, one line, actions; max 380 px, centred in the pane |
| Error / retry | `components/LoadError.svelte` (`.state-note--error`) | 72 px glyph on a risk wash, "Couldn't load …", reason, Try again + Copy details; filters and drafts kept |
| Unmounted route | Limits pattern (`_limits.scss`) | "This isn't available on this server yet" + key link; use for any admin route the API has not mounted `[lesson]` |
| Loading | `LoadingState.svelte` | see `foundations.md` |
| Toasts | `toaster.svelte.ts`, `ToastRegion.svelte`, `_toast.scss` | bottom-centre, never over focus; **at most two stacked, newest on top; success hides after 6 s, 10 s when it offers Undo**; errors stay until dismissed and offer the fix; announced via `LiveRegion` (`[old §Shared states]`, user decision 2026-10-05). **Planned fix (queued):** the code still stacks three and defaults to 8 s (`toaster.svelte.ts:38-41`). |
| Offline | service worker shell, `e2e/offline.spec.ts` | the app shows its own "You're offline" state ("Nothing private is stored on this device.") with Try again; the SW never caches `/api/`; the page-line chip turns Offline |

## Command palette

`components/CommandPalette.svelte`, loaded on first Ctrl/Cmd-K; commands in
`App.svelte` (including Experiments and overshoot toggles).
