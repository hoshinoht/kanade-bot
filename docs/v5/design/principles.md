# Design principles

The rules every admin screen follows. They come from the v4 portal (carried
in `[guide]`), the M3E handoff (`[old §Principles]`) and later user decisions.
Citation keys: see `README.md`.

## Identity: Kanade's Desktop

- A saturated coloured **ground** holds cream **windows** with solid title
  bars, rounded frames, restrained shadows and the three-dot motif. M3
  Expressive contributes layout, shape and feedback, not a palette (no M3
  dynamic colour). `[guide]`, `[old §Principles 1]`
- **Nothing legible sits on the bare ground** except the page line's title
  group, whose text uses `--ground-ink` (deepened per face for AA). Every
  other page-line item is its own chip, button or field. `[DR 2026-10-02]`,
  `_page-line.scss`, `_tokens.scss` header comment
- Don't nest cards where one window boundary already establishes the region;
  never nest a window in a window. `[guide]`, `[DR 2026-10-05]`
- **Boss art is scenery:** entry art is a faded, masked veil behind a card's
  text, never a banner or free-standing image; missing art renders nothing.
  `[guide]`, `--art-veil`, `--art-crop-*`

## Structure

- **One window per page, title-bar tabs** for peer views, counts on the tabs.
  No second row of window-level tabs. Pill tabs inside a pane are allowed (they
  belong to the pane). `[guide]`, `[old §Principles 2]`, `[DR 2026-10-01]` (O4)
- Accepted exceptions: Config's contents list (twelve-plus sections do not fit
  a title bar); summary cards inside a window where each summarises one peer
  object (Limits groups). `[guide]` (user decision 2026-09-25)
- **Fixed `100dvh` frame:** page line, title bar, tabs, identity and footers
  stay put; the window, pane, table or board column that owns the content
  scrolls. Each pane in a list-detail layout scrolls on its own. Operational
  detail pages: masthead/back/identity/tab strip never scroll; only the
  selected panel does, also on phones; never stacked card windows or body
  scroll. `[lesson]`, `[guide]`, `[old §Principles 3]`
- **List-detail instead of modals** on wide screens: selecting an item opens
  it in a pane beside the list; the list stays visible; the pane has a close
  button, Escape closes it, focus returns to the row. `[old §Principles 5]`,
  `[DR 2026-10-01]` (G4, G5)
- **Active-row containment:** the row whose detail is open gets the `--select`
  fill, a rounder shape (16–18 px), bolder text and `aria-selected` /
  `aria-current`. One row at a time; never bulk selection. Phones never grow
  selected rows. `[old §Principles 6]`, `packages/ui/src/media.ts`

## Hierarchy

- **Task first:** keep the current scope and primary state visible, give the
  working surface the remaining space, demote one-time explanation before
  shrinking data. Times and key counts are the loudest row values; member and
  boss names lead machine ids. `[guide]`
- **Area follows importance** (user rule 2026-09-25): screen area is a budget
  spent by importance; never add chrome above the primary surface without
  taking the same height from something less important. Per-page budgets live
  in each screen file; numbers in `foundations.md`. `[guide]`
- **One key action per pane:** the largest button, accent fill, fully round,
  always labelled. Destructive actions sit apart (gap or divider).
  `[old §Principles 7]`
- **One emphasised number per window:** a weight step in `--mono`, not a
  bigger size. The only hero size is the full run sheet's clock (`--fs-hero`).
  `[old §Principles 8]`, `[DR 2026-10-04]`
- **The boss week's shape:** wide screens use the seven-day board starting on
  the reset day and give empty days' space to busy ones; narrow screens use
  the seven-cell rail plus a run list. Scroll sideways rather than squeeze
  columns below legibility (230 px floor). `[guide]`, `_board.scss`

## Signals

- **Never colour alone** for navigation, status, selection or risk: pair it
  with text, weight, a ring, shape or symbol, plus programmatic state.
  `[guide]`, `[old §Principles 9]`
- **Shapes are accents, not content:** cookie, flower and burst only for
  portraits in lists, avatars, small badges and empty-state glyphs.
  `[old §Principles 10]`
- **Quiet motion:** short opacity/transform settles, springs on shape only,
  no layout animation, reduced motion honoured. `[guide]`, `[old §Principles 11]`
- **Real data only:** badges, counts, bars and chips show values the API
  supplies; values a board marks `[FROM YAML]` or "suggestion" stay off until
  the backend supplies them. `[old §The mockups are not code]`

## Interaction

- Frequent actions stay visible; labels name outcomes; every control is
  keyboard- and touch-usable. `[guide]`
- Preserve input after failures and focus after navigation and dialogs;
  loading, empty, denied, stale/offline, error/retry and destructive
  confirm/recovery states all exist in Kanade's visual language. `[guide] PWA behavior`
- Switches apply at once (with confirm when turning something off and a toast
  with Undo); form fields save with their section's save bar; actions run from
  their own button. `[old §Config]`
