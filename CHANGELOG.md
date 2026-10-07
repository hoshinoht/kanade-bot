# Changelog

Notable changes to the Boss Scheduler Bot, newest first.

## 1.0.0-beta.3 (in development)

**Added**

- Discord: a "Discord message style" setting (Config → Notifications, `notifications.message_style`, row `v5.message_style`): `classic` (default, every post byte-for-byte as before) or `redesigned`, read live per card in both attendance modes. Redesigned day-of cards post one embed per run (lead boss colour and portrait, catalog boss names, Discord timestamps, In / Waiting / Out side by side, run id and levels in the footer, entry art on the first run) with who is pinged in a subtext line; countdowns are coloured by state; the weekly digest is one ink-blue embed with a cleared bar and one line per run. The admin card preview shows every embed (`more_embeds`, field `inline`, embed `title`).
- Admin: chat allowances carry `resets_at`, when the member's oldest counted answer leaves the window; Limits → Allowances and Account → Chat allowance show "resets in 5 h 12 m", counting down on the server's clock. Limits shows each allowance as a bar of answers used with the window in words ("4 per 5 min").
- Admin: a "Reduce motion" switch on Account › This browser makes the app move exactly as under the device's reduced-motion setting (still shapes, flat bars, no pane, card or scroll animation), live and from the first paint. It is kept in this browser only; off, the app follows the device, which can still reduce motion on its own.
- Admin: Account › Sessions lists your own live sessions with browser and system, and signs out one or all others (`/api/admin/me/sessions`; store migration 0026 adds `web_sessions.device`, a short label derived from the User-Agent, never the raw header).
- Admin JSON reads carry a strong ETag and answer `304 Not Modified` to a matching `If-None-Match`; the admin app revalidates the reads that live hints trigger and keeps unchanged data on screen.
- Arrival motion for changes made elsewhere: Week cards glide to where another admin moved them, new Week runs and Inbox, Chat and Extractions rows are marked once, and changed Inbox counts pulse. Your own writes never animate this way, and reduced motion keeps only a colour fade.
- Admin: a Reminders row opens its Discord card preview (side pane, or a sheet on phones) through a read-only `GET /api/admin/reminders/{id}/preview`. Unsent cards are built as the delivery tick will post them; posted cards as the refresh worker edits them, from the runs they were posted for and with their stored heading. Stale rows show no card.
- Admin: History → Checkpoints names the first bad record when the chain check fails (`Verified.first_broken`), and its populated backups view is covered end to end.
- Admin: Discord links (Inbox evidence and cards, run sheet cards, sent reminders, last digest, chat cards) open in the Discord app via `discord://` by default; turn off "Open Discord links in the app" on Account to keep https links on that device.
- Admin pages update live: `GET /api/admin/events` sends change hints over SSE (schedule, inbox, chat, extraction, delivery, settings, rescan, members), fed by the store's write hook. Pages re-read in place and apply only newer data; untouched Config forms follow saved changes while edits in progress are kept. Polling slows to 60 s while the stream is open and returns to 15 s when it drops; hidden tabs close the stream, and a stream never extends the session's idle time.
- Admin: member and admin portraits. Discord avatars are cached by image
  hash on the data volume (`<KANADE_IDENTITY_DIR>/members`) and served at
  `GET /api/admin/members/{id}/avatar` and `/api/admin/me/avatar` (admin
  listener only, ETag/304, monogram fallback, purged when a member leaves);
  Members, the member sheet, Inbox threads, Account and the account chip show
  them. A Discord sign-in stores the avatar hash with the session (store
  migration 0025).
- History: Limits window clears are recorded (actor, time, member and the
  window as it was), exactly once per effective clear.
- Discord: `/debug reminders [run_id]` lists a run's reminder rows as v4 did,
  and `/debug materialise` materialises both weeks through the shared writer.
  `/debug tick` stays out (the delivery loop has no on-demand tick).
- Admin: a "Quiet mode on" chip (bell-off icon and words) takes the Live
  chip's place in the page line and phone top bar while quiet mode is on,
  updating as soon as Config saves; `GET /api/admin/summary` gains
  `quiet_mode`.
- Deploy: an optional cloudflared sidecar (`--profile public`) on an
  internal `kanade_public` network, reading its tunnel token from the
  `kanade-cloudflared` secret file; the bot joins that network but opens its
  public listener only when `kanade.toml` sets `[public] bind` and `host`.
  Runbook in `deploy/README.md` ("Public portal").
- Config → Channels: watched channels, watched categories and chat
  categories each save on their own and then override their env seed
  (`KANADE_WATCH_CHANNEL_IDS`, `KANADE_WATCH_CATEGORY_IDS`,
  `KANADE_CHAT_CATEGORY_IDS`); a list nobody saved keeps following its
  variable.
- Model logs keep the `x-request-id` correlation ids each chat round and
  extraction call sent (store migration 0024; older rows have none): the chat
  Model trace and extraction outcome show them, `chat_answered`/`chat_failed`
  log every id sent (failed requests included), and `models check --probe`
  prints them.
- Boss guides: shorter strategy names (e.g. "P4 burst only", "Keep P2 mark-free")
  from the 2026-10-05 strategy review; steps and facts unchanged.
- Boss guides: the HP breakdown starts folded, showing only "Total HP" in its
  head until opened.
- Admin Account page: `GET /api/admin/me` (admin listener only) shows how you
  signed in; a Discord sign-in also shows its chatbot access, bossing role,
  server role names and the same chat allowance Limits shows, while the admin
  token and Tailscale show a neutral "Not a Discord member" note. Opened from
  the account menu's new "Your account" item; "Copy user ID" now reads the id
  from `/me`.
- Boss guides, second pass: the Recommended tile lists one figure per party
  size (`recommended_spec.parties`), phase cards are fuller, the HP breakdown
  folds away, the phase bar is a connected button group, an "On this page"
  list sits at the top of the aside, the header shrinks to portrait, name and
  difficulty while scrolling, the guide ends with room below its last card,
  and Destiny/Champion lose their rim in the difficulty switch. Malefic Star's
  1000 lock ends in a forced swap after 40 s; Baldrix Hard lists trio, duo and
  solo figures.
- Wiring sweep: an inventory of what the PWA, mock, docs and v4 offer
  against v5 `serve` (`docs/v5/admin-api.md`) and a route-diff script
  (`scripts/api_routes/route_diff.py`); the dev mock now refuses Re-read while
  the extractor is off, as the server does, and Preview ping says plainly that
  nothing is posted.
- Bosses knowledge page is a shorter tabbed guide (Overview, Phases,
  Strategies, Notes, Sources; `?tab=` and `?phase=` deep links): a mission
  card with its series track (Destiny numbers, Union Champion rank letters),
  fact tiles, an HP breakdown by phase and target, one-line titled bullets,
  mechanic cards, a phase timeline that picks one phase (repeating groups
  marked), and strategy cards with folded steps. All 13 guides are rewritten
  in this shape with every old fact kept in the text or the chatbot-only
  detail; Black Mage gains Black/White State strategies and Baldrix the
  forced-rotation room order.
- Boss guide contract for the redesign: the knowledge API lists every boss in
  a guide's mission series (`missions`, by order); the chatbot reads titled
  items with their full `detail`, phases (with timeline group and tag),
  mechanic blocks, mission lines, HP targets and `×count`, and spec figures;
  `validate.py` fails shared mission orders, duplicate phase names or
  mechanic titles, more than one mission per series and split phase groups.
- v5 web e2e `clipping.spec.ts`: every admin and public screen (plus Config
  sections, Inbox items, Week tabs and an open run) is audited at the five
  layout frames for cut, clipped, spilled and off-screen text, with one
  allow-list of intended ellipses. Fixes what it found: the Inbox thread's
  Used / All switch no longer falls off a narrow thread (the head takes two
  rows), the thread foot and the phone item head wrap their facts so "Read
  from chat" and "See the card" are never cut, the Chat turn's facts wrap
  instead of hiding the token counts, and the Persona table's default prompt
  wraps instead of "(the pe…".
- Boss guides gain Destiny Weapon mission entries (Seren, Kalos, Carling, First
  Adversary, Limbo, Baldrix: modifier, Adversary's Resolve cost and per-clear
  Resolve) and Union Champion trial entries (Lotus, Black Mage, Seren, Kalos).
  Carling uses Tiger/Bird/Dog callouts with Tiger + Bird and Bird + Dog break
  routes; First Adversary adds the Stage 5 opener.
- v5 admin History lists Config section saves: every effective save (persona
  switches included) is stored with its changed settings before and after
  (store migration 0023) and shows as a view-only "Config" row in the
  timeline, with the field diff, raw JSON and a link back to the section.
- v5 admin Limits: one full-width card per model group (permits bar,
  breaker in words, rate and retry budgets, models, waiting calls), Queue,
  Admission (refusals by backend group and gateway key) and Allowances tabs;
  phones list open and half-open breakers first and scroll the tab strip
  sideways. The footer prints the server's clock from a new `generated_at`
  on `GET /api/admin/limits`, and a failed refresh shows a Retrying chip
  (on phones under the tabs) that says when refreshing stops and offers Try
  again; a failed first load shows the shared failed state, and focus moves
  to the member's name after a Reset.
- v5 admin run pane and run sheet: a "View weekly timing" link on runs that
  came from a weekly timing opens it in the Fixed editor.
- v5 admin Fixed editor: the Time is the Move picker's stepper (Run lengths
  step), still typed into, with Enter saving as before.
- v5 chat profanity guardrail: the nudge deny-list (plus Config's extra
  words, minus words allowed again) checks member questions, answered with
  a configurable in-character line and no model call, and the model's own
  reply words, which get one clean retry before the line is sent. Member
  names, run data and meso amounts never count; deflected exchanges stay
  out of later chat context. Hits log the Chat outcome `profanity` with the
  side, word and line sent; Config gains a live `profanity` section.
  Reminder heading rewrites check the same live list.
- v5 admin Profanity: Config → Profanity edits extra blocked words, built-in
  words allowed again (found by typing, never listed whole), the question and
  reply checks (live, with Undo) and the deflection line; Chat filters the
  `profanity` outcome and each such turn shows the side, the matched word and
  the line sent (or "Retry answered cleanly").
- v5 admin dropdowns: every select is one rounded pill with keyboard
  type-ahead, search on long lists and the phone's own picker on small
  screens; Re-read channels is a multi-select (All · None), and the Dates
  range sits in the Chat and Extractions filter row.
- v5 admin Reminders: the Run filter lists runs by day, each with its time
  (for the week on the board) and how many cards are still queued.
- v5 admin Move picker: a boss-week day strip and a time stepper (Run lengths
  step) with the typed "wed 21:30" shortcut, suggestions and clash warnings,
  in the run pane, the run sheet and the Inbox's "Edit, then approve"; the
  Fixed editor's Day is a weekday strip.
- v5 admin sign-in page shows tonight's next run (time, bosses and the yes
  tally; no names) from a new sessionless `GET /api/admin/auth/tonight`.
- v5 admin date picker: Chat and Extractions date filters and History's
  "Since" use a Thursday-first month grid (a boss week is one row) with quick
  picks, typed From/To with errors, keyboard control and a phone sheet; today
  comes from the server clock.
- v5 admin Reminders follows its M3E board: Queued / Sent / Stale & other
  tabs in one window, a Filters (n) popover, one day-grouped table with an
  "In" column and party chips, and a footer naming the next card. Only the
  table scrolls; paging is gone.
- Config › Models › Capacity reports each group's in-flight permits
  (`models.groups[].in_use`, `null` when the governor does not run that group),
  and its bar waves while calls run. The Limits permit bar keeps waving when
  every permit is held by calls in flight (still at most two waves per screen).
  The mock's Config and Limits now name the same model groups.
- v5 admin PWA progress bars for everyone: the boss-week day in the Week
  footer, run countdowns (wavy over the final 24 h, with T-1h/T-15m marks),
  answer bars on run cards and the run pane, proposal expiry in the Inbox
  (warning colour near the end), and model permits on Limits and Config. The
  Re-read bar no longer sits behind Experiments; at most two bars wave per
  screen and reduced motion draws them flat.
- v5 admin PWA run change log: the Week run pane's Changes tab (and the foot
  of the phone run sheet) lists the run's changes newest first, like an IDE
  commit list: a one-line summary over avatar · actor · time ago · via
  surface · #seq, each row opening to the run's fields before → after, with
  a "By field" filter in place of the old blame table. The History timeline
  draws its records on one rail, tagging the records backups anchor, each
  rollback with what it reverts, and each undone record with its rollback.
- v5 admin PWA Re-read card shows a running job's progress in messages:
  "41% · 27 of 66 messages · started 01:40" (guild time), with the wavy bar
  (experiment B) counting messages too. The percentage and count are left
  out while the total is unknown or zero, the start while queued; the
  percentage never passes 100%.
- v5 admin rescan jobs report progress: `started_at` and the job's
  `messages` / `messages_total` (gated messages read, and the window's
  messages counted from the cache when the job starts), so the Re-read card
  can show "41% · 27 of 66 messages · started 01:40". The PWA mock serves
  them too.
- v5 admin API: `GET /api/admin/history?run=<id>` pages one run's change
  log, newest first: every record that changed the run or its RSVPs
  (creation, moves, swaps, status, attendance, rollbacks), each with its
  actor, surface, time, seq and the run's rows before and after. The pwa-mock
  serves it too.
- v5 admin and public theme pickers group colourways into collapsible sets:
  Base (Otonose, Nazuna, Sumire, Hinano), Blue Archive (Hoshino, Mika, Seia,
  Hina, Aris), Terminal (Catppuccin, Tokyo Night and GitHub, from their
  official light and dark palettes, nudged only where contrast checks failed)
  and Dynamic (a palette from the bot's avatar, falling back to Otonose). The
  current colourway's set starts open. Coral is retired and a stored choice
  falls back to Otonose; Ctrl-K still finds colourways by their stored keys.
- v5 admin Inbox has a read-only **Past** tab listing closed proposals and
  member requests newest first, each with its outcome, who decided and when,
  the reason, cited Discord messages and links to the card, its History record
  and the log entry that staged it; older items load on request.
- v5 admin API `GET /api/admin/inbox/past` lists closed proposals and member
  requests newest first (outcome, who decided and when, reason, source with
  message links, History record for approvals), paged by `before`/`limit`;
  the dev mock serves invented closed items for the coming Inbox Past tab.
- v5 admin Week matches the M3E boards: one Week window (Planner / Runs /
  Answers tabs, This/Next, a Filters popover and a status footer), an
  at-a-glance pane from 1200 px, the run as a side pane from 840 px (Run /
  Answers / Changes), a Runs table, HTML answer bars with a table toggle and
  two-line phone run cards. Planner cards grow on hover or keyboard focus
  (not on touch), a click opens the run, and clicking outside the run sheet
  closes it unless an edit is in progress.
- v5 boss animations: `/art/animated/{key}` serves the boss's MP4 from
  `boss/artwork/animated/` with byte ranges (so Safari and iOS can play it),
  boss knowledge and event bosses carry a nullable `animated` URL, and the
  content security policy allows same-origin media. Discord cards keep the
  still art.
- v5 admin Bosses knowledge hero plays the boss's looping MP4 muted over its
  still entry art; under reduced motion, or if the video fails, the still
  shows instead. The admin service worker leaves `/art/animated/` to the
  network so byte-range playback works.
- v5 admin M3E static motion: buttons morph on press, selected rows spring
  their corners, side panes (Members, History, Fixed, Inbox, Bosses) slide in
  and out, dialogs and toasts fade out, the phone Inbox moves forward and back
  between list and detail, loading indicators appear after 200 ms, and your
  own Week moves glide into place (other admins' changes do not); planner
  overshoot is opt-in with `?overshoot=on`. Reduced motion stays instant.
- v5 chat answers schedule questions in the persona's own words: the model
  cites runs by id, grounding shows each cited id as the run's boss name and
  puts the run's card under that paragraph without the listing heading, and
  personal results give the model hidden context (time until the run, the
  asker's own answer, who has not answered) that members never see raw. A
  citing line that states any schedule fact beyond that context becomes the
  card; the rule is deliberately simple and will be tuned from live traces.
- v5 admin Config: Pings countdowns are chips with × to remove and an Add
  field (a duplicate is skipped); Re-read uses channel checkbox chips and a
  running-job card (progress, what it found, Cancel), also on Extractions;
  Theme uses colourway tiles and a Light/Dark/System group; Enter in "Find a
  setting" jumps to the matching card, opening its sub-tab, and outlines it.
- v5 admin Bosses knowledge pages show each boss's strategies (when to use it,
  risk, damage needed, payoff and steps) when its document has them.
- v5 `kanade backup [--name FILE]` snapshots the stopped store (`VACUUM INTO`,
  0600) into `KANADE_BACKUP_DIR` with its manifest, which now records
  `created_at`; it refuses while the bot owns the store and never overwrites.
  `GET /api/admin/history/checkpoints` lists those manifests newest first,
  re-checking each head against the chain on every request (`anchor`:
  `matches`, `older_schema` or `mismatch`) and reporting
  `backup_dir_configured`. Compose mounts the backups directory read-only into
  the bot and adds a `backup` tool service; the deploy runbook takes the
  snapshot after stopping the bot, beside the volume tarball. Mirrored in the
  dev mock and API types. The admin History → Checkpoints tab (B_HistoryCk)
  shows a verification card ("Chain verified · n records · head · checked
  just now", or "Chain check failed" in the risk wash) with **Verify again**
  (a read-only re-check, announced politely) over a backups table whose
  Anchored column reads ✓ matches, ○ older schema or ✗ mismatch in words;
  with no backups it says none were taken yet, or that this server has no
  backup directory.
- v5 admin Config API adds three read-only fields for the Config page:
  `models.capacity_check[].group` names the capacity group each check is about
  (null for cross-group checks), `env[].copy` gives the raw value to paste into
  the deployment env (null when unset), and `last_digest` reports the newest
  active weekly digest (guild-offset `posted_at`, boss-week start, whether it
  is this week, channel name and message link). Mirrored in the dev mock.
  The admin PWA shows them: Models → Capacity puts each group's verdict in a
  Startup check column (cross-group checks stay listed under the table),
  Weekly digest gains a "Last posted" card (guild time, week, channel, a
  Discord link when known; omitted without a digest), and each env row with a
  raw value gets a "Copy KANADE_…" button. Persona's reply profiles lead with a
  fixed "Default voice" row (the active persona as written; never selected or
  saved). The dev mock's env rows now use the server's keys, labels and order.
- v5 admin Inbox items carry an optional one-line `consequence` read off the
  merge preview (e.g. "Party unchanged · 2 reminders will move", "Adds Finn",
  "2 reminders will be dropped"): party joins and leaves of the changed runs
  and the upcoming reminders that move, drop or appear; `null` with
  conflicts, expiry, no effect or nothing to say. Served by the pwa-mock and
  typed in `@kanade/api-types`; the decision card shows it before Approve
  (inside the change card on phones).
- v5 admin API reads and changes a weekly timing's owner: `FixedRow` adds
  `owner_id`, and `POST`/`PATCH /api/admin/fixed` accept an optional `owner_id`
  (a rostered, non-bot member, not necessarily in the party; else `422 invalid`).
  An owner change is blamed as `owner`, conflicts like other timing fields and
  survives idempotent replay after the owner loses the bossing role. The admin
  PWA Fixed editor shows an Owner select beside Day and Time (stacked on
  phones), preselects the saved owner, defaults a new timing to the signed-in
  Discord member or else the first party member picked, and reads an owner
  refusal out on the field.
- v5 admin phone navigation drawer shows Members and Reminders counts beside
  Week: `GET /api/admin/summary` adds `members` (bossing roster) and
  `reminders` (upcoming cards), derived as the Members and Reminders pages count.
- v5 admin API removes a chat alias: `DELETE /api/admin/members/{id}/aliases/{alias}`
  drops it from the member (others keep their order) and frees it for anyone;
  removing an alias the member does not hold returns the unchanged row. The admin
  PWA Members sheet gives each alias chip a × that removes it in place.

**Changed**

- Web CI: the e2e suite is steadier on the 4-vCPU runner. Motion tests run in their own one-worker step with frame-rate floors reported, not enforced, on CI; the long accessibility walk and Config save tests are split; capture tests get 120 s; the text audit ignores spaces hanging at a pre-wrap line end; three timing races are fixed (queued reminders, the 200 ms loading delay, the pane exit); failure traces are now uploaded (`include-hidden-files`).
- Web: the admin and public PWAs are served precompressed. The web build writes `.br` (quality 11) and `.gz` (level 9) siblings for JS, CSS, HTML, SVG, manifest and JSON files of 1 KB or more, kept only when smaller and never precached by the service worker; the server picks br, then gzip, then the plain file by `Accept-Encoding` q-values, the SPA shell included, with `Vary: Accept-Encoding` and unchanged cache headers. The admin entry script drops from 160 KB to 47 KB on the wire on both origins. Direct requests for `.br`/`.gz` files are 404.
- Admin: the Account page is rebuilt as one window with Profile / Sessions / This browser tabs: access and a recheck, an allowance meter, the reply style in effect next to your saved one (with a searchable picker of public styles), diagnostics, and per-browser look, Discord links and shortcuts.
- Admin: Reminders "In", "today" and "Next in" follow the server clock (`generated_at`, `fire_at`), not the browser's.
- Admin: Re-read buttons on the Week board, run pane and sheet, Extractions
  and Config → Re-read are off with the server's reason shown before any
  press while extraction is switched off, or when the server runs no
  extractor; `GET /api/admin/summary` gains `rescan_off`, and the rescan 503
  now carries the same no-extractor sentence.
- Admin summary: `model` reports busy when a governor group has every permit
  in use, naming the longest-held permit's kind.
- CI runs the API route diff (`scripts/api_routes/route_diff.py`) in the web
  job.
- Admin: Chat, Extractions, History, Bosses and Config show the standard
  "Couldn't load …" pane (reason, Try again, Copy details) when their first
  read fails, and Members, Reminders and History show loading words while
  their first read is slow, as the other pages already did.
- Boss guides: accuracy pass from the 2026-10-05 strategy review. Limbo
  fusions, Baldrix Phase 2 pillars and Ragnarok timing, Black Mage i-frames
  and Destruction bonus are corrected or marked unverified; Malefic Star
  altar arithmetic is flagged unresolved; kill orders, margins and side
  assignments in Carling, Seren, Kalos, First Adversary and Jupiter read as
  party conventions; Carling's Tiger + Dog break is "disfavoured", not dead.
- Boss guides: each party size's recommended figure is its own fact tile
  ("Recommended · Trio", "Recommended · Duo", …), each with its basis.
- Difficulty pills: Easy is a slate pill (white on dark grey; light grey with
  dark text at night) so it no longer blends into grey cards.
- Docs: the v5 contracts, design spec, decisions and evaluations moved from
  `docs/v5/` to the private `docs/notes/`; tracked code and docs now point there.
- The dev-only PWA mock server moved to `devtools/pwa-mock/` (was
  `tools/pwa-mock/`) and is labelled as developer tooling that never ships.
- `docs/v5/` now publishes only the setup guides (`runtime-bootstrap.md`,
  `v4-import.md`) and the test data (`vectors/`, `api-schemas/`); contracts,
  design specs and evaluations are git-ignored and kept locally.
- The frozen v4 rollback tree `legacy/python/` is no longer tracked (git-ignored,
  kept locally; in history up to `487c4ed`): CI drops the Python and v4 image
  jobs, the legacy-only git hooks are removed, and the boss-knowledge
  validator reads the root `boss/bosses.yaml`.
- Upgraded `scripts/bench_headers.py` with automated scoring, multi-repetition capabilities, and markdown reporting.
- v5 PWA: at most two toasts stack, newest on top, hiding after 6 s (10 s
  with Undo) while errors stay until dismissed; every admin list-detail
  screen and the Week run pane switch to one pane at a shared 900 px
  breakpoint (Bosses gains a Back link in the rail frame); Zilla Slab and
  Maple Mono load real 700 faces instead of synthesised bold.
- Boss knowledge accepts Champion and Destiny difficulties, shown only on the
  Bosses info page (orange and dark-rimmed ticks, info only) and in chat
  guides, never in the scheduler.
- v5 admin run pane and run sheet: boss art stays inside the identity card
  and fades into it (no hard cut behind Move), only where there is no text:
  behind the actions on the laptop sheet, a top-right corner by the clock in
  the pane and on phones; two or three bosses show as angled slices in run
  order, further bosses as portraits only. Text contrast over it is measured
  from pixels in e2e.
- v5 admin full run sheet is one window: the identity card sits flush on the
  modal's surface (no ground band around it) and Party / Answers / Cards /
  Changes are a tab strip in it, not a second titled window.
- v5 admin sign-in and the full run sheet follow the M3E hero boards: a
  banner with the bot's name and a 52 px Discord key; the sheet's laptop and
  phone views get an identity card with a large clock (new `--fs-hero`
  tokens), Move and status, over a Party / Answers / Cards / Changes window
  with one row per member.
- v5 admin empty Inbox tabs say why nothing waits (the extractor's last read,
  links on, and the tab's last three decisions); failed panes offer Try again
  and Copy details; the phone top bar and drawer follow the M3E spacing.
- v5 admin Chat and Extractions: the Filters button has an icon and its panel
  closes on Escape or a click outside; on phones, Back closes an open
  extraction call, and the filter chips and panel stay inside the window.
- v5 admin Chat follows its M3E boards: one window lists interactions beside
  the open turn (`/chat/:id`), with Copy transcript, pill tabs, conversation
  bubbles and per-round Model trace cards; phones show the list, then the turn
  with "‹ Chat".
- v5 admin Extractions follows its M3E boards: one Calls window lists calls
  beside the open call (Changes / Chat read / Prompt / Raw tabs, an outcome
  card, and a code viewer with find, Wrap and Copy), with Filters and Re-read
  on its title bar. `/extractions/:id` links open that call in the list.
- v5 admin Limits: when the server has not mounted the limits route, the page
  shows its M3E board's unavailable state: one window with a centred glyph, a
  short explanation and an **Open Config → Models** link.
- v5 admin Week board: busy day columns keep v4's 230 px floor instead of
  squeezing to 184 px, so pills and names fit; a busy week scrolls sideways
  and the board fades at whichever edge has more columns.
- The web apps' API response types are generated from the Rust DTOs with
  ts-rs (`web/packages/api-types/src/generated.ts`, checked by a Rust unit
  test that fails when the file is stale); vocabularies, request bodies and
  the responses still built with `json!` stay hand-written in `manual.ts`.
- The admin log, rescan, limits and history responses and the API-owned
  vocabularies (inbox tabs and flags, card kinds and states, reminder and job
  states) are typed Rust structs and enums, so their TypeScript types are
  generated too and `manual.ts` halves. Fields and values are unchanged; keys
  in those responses now follow struct order instead of alphabetical order.
- CI runs each suite only when its inputs change: legacy Python and its
  image for `legacy/python/`, Rust for `src/`, `tests/`, Cargo files and the
  tracked data its tests read, web for `web/`, `tools/pwa-mock/`, boss
  knowledge and API schemas. Editing the workflow runs everything.
- v5 admin Week: clicking the open run's card or Runs row again closes its
  side pane.
- v5 admin Week: At a glance shows the next run's party with each member's
  answer, waiting first; the run side pane is wider (about 410-480 px from
  1000 px) and can pop out to the larger run sheet on the same tab.

**Removed**

- The reserved `kanade export` command.
- Removed `GET /api/admin/runs/{id}/blame` (and its mock route, schema and
  `BlameEntry` type): the admin PWA reads a run's change log from
  `GET /api/admin/history?run=<id>` instead. Field blame stays in the domain
  for write preconditions and cherry-pick.

**Fixed**

- Reminder cards: persona-voiced day-of headings and countdown/digest phrases are rewritten ahead of time by a background pass (cards firing within 12 h, the coming week's digest; 30 s per rewrite, at most 4 per minute) instead of at send time, where the 2 s budget almost always fell back to the plain line. A send never calls the model: it uses the stored line, else the plain one, which then stays. Logs gain `stage` (`pregen`/`send`).
- `serve` shutdown fits inside Docker's 30 s stop grace: every step shares one 25 s budget from the signal (3 s kept for closing the store, of which chat's final log writes may use up to 1 s, so the close always has at least 2 s), a Discord send still in flight at the cut (the tick, a manual digest, card posts and edits, decline retraction) counts as possibly delivered, is journalled before the store closes and is never re-sent, a header pre-generation rewrite still in flight ends at the cutoff and stores nothing (the send keeps the seed), a store write it began finishes (unless the worker is aborted at the cutoff, which rolls that write back whole), store reads stop at the cutoff so a busy read pool cannot hold the delivery tick (waiting for a read connection is also capped at 5 s instead of 30 s; an admin read answers 503 `unavailable`), stuck workers are aborted, and a step the budget cuts is logged `shutdown_deadline_cut` instead of failing the exit (an outside process holding the SQLite write lock can stretch this to about 27 s).
- After a server restart (for example a backup restore that lowers the history head), open admin pages take the restored data instead of keeping the pre-restore week until reload.
- Store: migration 0006 is restored byte for byte (a comment path edit in the
  docs move changed its checksum, so an existing store refused to open); a
  test now pins every shipped migration's checksum.
- Saving a Config switch no longer freezes the env-seeded channel lists into
  stored settings.
- v5 chat: a `request_tools` call with the wrong argument shape is now told
  the argument name (`Call request_tools with {"bundle": "strategy"}; …`), so
  the model recovers on its next round instead of wasting several.
- v5 chat: withholding a content-filtered message no longer drops every
  unrelated re-anchorable exchange; only anchors tied to that message go.
- v5 admin Chat: the date presets in the filters panel are visible again.
- v5 admin run countdowns no longer bunch their marks at the end: the bar fills
  from 24 h out to T-1h, then restarts over the last hour with the T-15m mark
  at three quarters. Opening the run pane scrolls the board to keep the
  selected card in view.
- v5 admin run pane: the countdown's track and T-1h/T-15m marks stay visible
  over the boss art.
- v5 admin Week Glance: the "Next up" card is readable over bright boss art
  (stronger art, a scrim behind the text, a darker countdown track and ticks),
  and the fill line keeps its space before "maybe". On Next week it still shows
  the next run's portrait and countdown, and Open sheet switches to This week.
  A busy reset-day column head no longer runs its count into "reset".
- v5 admin and public PWAs: the fixed frame clips instead of hiding overflow,
  so focusing or revealing a control (opening a Config section) can no longer
  scroll the whole shell up under the window.
- v5 admin Week: the selected run card keeps its artwork and status bar inside
  its rounded corners, and a hovered card grows in place and pushes the runs
  below it down instead of covering them.
- v5 admin Week: the channel filter now filters (it compared the channel id
  with the channel name the API sends as `party`) and lists only the shown
  week's party channels.
- v5 admin run pane: the Changes tab stacks each change so nothing is cut off
  at the pane edge, and the home channel shows its name; full boss tags no
  longer repeat their name in a tooltip.
- v5 chat keeps the persona's schedule wording and puts the run's card under
  it; it replaces a line only when it names a run id, time or date the
  schedule lookup did not return, instead of falling back on any word the
  checks could not read.
- v5 chat "when is my next run" lists every upcoming run from now across boss
  and calendar weeks: `get_schedule` with `week:"auto"` and no day no longer
  stops at Sunday, so runs early next calendar week (still this boss week) are
  found instead of "No upcoming runs for you in this week".
- v5 chat keeps a persona sentence that mentions a run and puts the run's
  record under it, instead of replacing the whole sentence with the record.
- v5 chat "when is my next run?" answers with just the soonest run, and
  `get_schedule` `week:"auto"` without a day stops at the end of this boss
  week instead of listing every later week; plural or qualified asks ("my next
  runs", "next run for hard lucid") still list this boss week's remaining runs.
- v5 chat also keeps a persona sentence that names a run by its date and time
  (no id) only when every fact in it was read and matches the run (fail-closed),
  with the run's record under it; another catalog boss or difficulty, a wrong
  or unread number, date, tally, status or channel (also in words), a
  negation, am/pm, a relative day such as "tonight", or more than one time
  still gets the listing. Not caught: a plain name of a boss outside the
  catalog, a lowercase everyday-word alias ("star"), member names, and facts
  phrased in words outside the checked lists.
- v5 admin Bosses event rows, when selected, show the boss's difficulties and
  levels below the name like catalog rows, instead of an unwrapped availability
  note that ran past the row.
- v5 chat no longer treats an ordinal that cannot be a date ("waiting on its
  first ✅") as an unread fact, so such a persona sentence keeps its wording.
- v5 avatar initials (Members sheet, inbox transcript, rail, drawer, account
  menu, sign-in and public masthead) show the first readable letter of a name,
  skipping emoji and symbols: "🥔猫铃薯🥔" shows "猫" instead of a broken half
  of the emoji.
- v5 admin phones keep selected list rows on their compact line across Fixed,
  Week, Inbox, History, Members, Bosses and Config, including landscape; the
  sideways settings strip never grows, while desktop rows still expand; the
  Inbox heading wraps each boss name with its pill as one unit, and a Members
  name cell keeps the name whole, ellipsises aliases and drops the "chat only"
  chip below the name rather than past the column.

## 1.0.0-beta.2 (2026-10-02)

**Added**

- v5 admin Bosses now uses the M3E catalog and checked-in knowledge workspace:
  level-ordered boss rows keep weekly difficulty ticks visible beside the selected
  strategy, facts, provenance and seasonal availability, with this week's linked
  timings alongside. Phone navigation moves from the catalog into a backable detail.
- v5 replays offline ✅/❌ on pending proposal cards after startup and fresh
  gateway READY, using current approver roles; conflicting answers stay pending
  with a visible note, and stale chat rejection follow-ups are not sent.
- v5 model logs retain Kanata reasoning text (64 KiB, visibly truncated) and
  reported reasoning tokens; admin Chat/Extractions show collapsed reasoning
  and token counts, and copied chat transcripts include both. Oversized counts
  stay unknown, and retries keep a single truncation marker.
- v5 admin Inbox items read closer to their boards: the header and thread
  panel form one column beside the decision pane, a proposal shows Kanade's
  one-line italic summary under its header on wide screens, the facts line
  is smaller with the read time in body type, the phone header is tighter,
  and the "Can't approve" line appears only when Approve is held back. Wide
  Extractor items follow VarRail2: the header (larger art, summary, a
  confidence burst) runs across the top, the thread fills the height beside
  a 300 px decision card that holds the change (the new time large, under
  its date), with the thread's channel and time span in its bar, the facts
  and "Open in Discord" at its foot, used messages marked "used", and
  "See the card" beside Reject; Extractor list rows lead with the boss art
  and say how much of the thread was used.
- v5 admin Config sections now follow their M3E boards: each section has a
  heading and lead, settings cards that scroll, and a save bar that stays put
  (dirty dot, "field old → new", Discard, Save <section>; disabled when clean,
  and the contents list marks a section with unsaved changes). On/off settings
  are switch cards that apply at once, ask before turning off (before opening
  the public portal), and offer Undo for 10 s. Persona splits into "Active
  persona & profiles" / "Role overrides" pill tabs and Models into "Roles" /
  "Context windows" / "Capacity"; "Use this persona" and "Post it now…" confirm
  first. The Context windows "In effect now" table has its own card heading
  above the table, and Re-read spaces its channel chips and Window row like
  B_CfgReread, with a "Channels · n of m" count, Select all / Clear and a
  one-at-a-time note.
- v5 admin Inbox items now carry `thread`: the stored channel messages around
  a proposal's evidence (oldest first, at most 37, the extractor's context plus
  a burst), each marked `used` when the proposal cites it. It is read from the
  watched-message cache, so deleted or pruned messages never reappear; member
  requests and proposals without a card have `thread: null`.
- v5 Discord chat now answers boss strategy questions from the checked-in boss
  knowledge (`KANADE_KNOWLEDGE_DIR`), without sources, as v4 did; without a
  knowledge directory the strategy tools are not offered at all.
- Boss guides refreshed for MapleSEA (researched 2026-10-02): SEA names first,
  full per-difficulty facts for every boss, and the KMS HP values both before
  and after OVERDRIVE. Guides gain named strategy options, each with its risk,
  damage requirement and payoff (e.g. four Radiant Malefic Star altar routes).
  Chat can now answer event bosses by name or alias (new Meilin prep guide;
  Bellona is kept as a prep guide until she reaches MapleSEA).
- v5 admin Bosses shows event bosses (e.g. Kai, Meilin) with their portrait
  or icon like catalog rows and a "Seasonal boss · Challengers World Season n"
  chip, also on their knowledge page with the portrait. `/art` now serves art
  for a key an event knowledge document declares (exact case), and the events
  read carries `portrait`, `portrait_sm` and `art`.
- v5 admin Config now uses the M3E Settings window: a grouped, deep-linkable
  contents list with current-value hints, a "find a setting" search, settings
  cards in the section panel, and a page-line risk chip (Config only) in place
  of the Manage Messages banner, linking to Channel access.
- v5 admin Limits page and "Post digest" now work against the real server:
  limits are read live, a usage window can be cleared, and a manual digest for
  this or next week posts through normal delivery to a channel the bot knows
  (replacing that week's digest, as in v4). Config-change audits now name the
  acting admin.
- v5 admin History now uses the M3E audit timeline list-detail layout, with a
  side-pane change review on wide screens and a full-screen detail sheet on phones.
- v5 posts v4-exact decline notices again: a ❌ reaction replies to the card,
  `/rsvp no` and portal declines go to the run's home channel, and extraction
  declines post in the source channel, with v4's cooldown. Changing the answer
  back deletes the notice, even if it was still being sent. Chat proposal cards
  and v4 import post none. Store schema v20: rollback needs the previous image
  plus a pre-v20 backup.
- v5 admin Fixed now uses the M3E weekly-timing list-detail layout: the editor
  is a side pane beside the timings on wide screens and a full-screen sheet on phones.
- v5 admin Members now uses the M3E roster list-detail layout: the member
  editor is a side pane beside the roster on wide screens and a full-screen
  sheet on phones.
- v5 admin Members matches its M3E board: one framed roster with bare run
  counts and capitalised @mention levels, a detail header with the member
  overline, handle and an icon close button, connected @mention pills, alias
  chips, and a This week list of the member's runs; "Sort: runs" orders the
  roster by runs this week (A–Z between equals) and can switch to A–Z; phones
  drop the reply-style column so the roster never scrolls sideways.
- v5 now writes v4-exact weekly-timing added and removed notices with their
  deciding commits (never during v4 import), and refreshes posted proposal
  cards after committed portal inbox approvals or rejections.
- v5 admin Inbox now uses the M3E contained list, thread and decision-pane primitives across Extractor and Self-service, including the approved phone detail action bar.
- v5 chat now follows up a qualifying ❌ on its own proposal card with one
  read-only, silent reply to the card. It verifies the original chat asker,
  uses a per-channel cooldown, spends no chat allowance and retains only the
  visible assistant answer in channel context.
- v5 admin planner cards can swap directly by dropping on another live card,
  or with `S` during a keyboard lift; the run sheet also provides an accessible
  picker with a two-run preview and one-step undo.
- v5 admin planners can atomically swap two live runs' slots in one boss week,
  with one history record, reversible as one change and per-run move notices.
- v5 run lengths are a live, saved setting (`v5.run_lengths`, seeded from
  `[settings.run_lengths]`/`KANADE_RUN_LENGTHS` until saved): a default per
  boss (30 minutes) plus per boss/difficulty overrides (Hard Black Mage 60);
  invalid seeds refuse startup. Admin week runs carry their length in minutes
  (the sum over their bosses), the groundwork for planner drops that set the
  time.
- v5 logs token usage: schema v19 adds nullable provider-reported
  prompt/completion tokens and the local prompt estimate to every extraction
  call (summed over the attempts that reported a pair) and every chat round
  (`null` = not reported; older rows and v4 imports stay `null`). The admin
  API shows them on chat rows, turn rounds and extraction rows/details (with
  the call's context window), and per-model chat and extraction summaries add
  reported sums and the median reported/estimate ratio. Rolling back to a
  v18 image requires the paired pre-upgrade volume backup.
- v5 chat shows the persona's staging line as a silent reply with typing
  while it answers, then edits that message into the answer; longer answers
  continue as silent follow-ups. A deleted question's placeholder is
  withdrawn, an answer that stops early ends with `*(reply incomplete)*`,
  shutdown turns a pending placeholder into the failure line, and chat-log
  rows record what was delivered (`guardrail.delivery`, `incomplete:
  delivered k of n parts`).
- v5 model context windows are live, saved settings (`v5.model_context`,
  seeded from `[models.context]`): per-alias override, then the catalog's
  published window, then a cloud (65,536) or local (8,192) default, capped at
  131,072 and by optional per-role caps. Per-role reply reserves (chat 1024,
  extraction 2500, rewrite 96) are sent as `max_tokens`, clamped to Kanata's
  published `max_output_tokens`. Config API, `kanade models check`, startup
  logs and model-log rows show the effective window and its source, and warn
  when a local model's window exceeds 16,384 or a reserve fills its window.
  Changes apply to the next chat question, extraction pass or rewrite without
  a restart; the fixed 65,536 serve window is gone. Config → Models has a
  Context windows panel with paired sliders and number fields, per-model
  override rows showing published maxima, and the local 16k warning.
- v5 chat trims oldest history, then older tool results, before failing a
  too-long question with a typed context-budget error and a member reply;
  a Kanata size 400 above the published output maximum no longer downgrades
  the model's sampling controls.

**Changed**

- v5 admin selected list rows smoothly grow to reveal their full content across
  Fixed, Week, Inbox, History, Members, Bosses and Config; compact multi-boss
  timings use overlapping portraits, and planner days always keep one card column.
- v5 chat reuses another channel's live proposal for the same run change,
  pointing to its existing card (or saying it is still being posted) instead
  of creating a duplicate, while retaining normal retirement and card
  refreshes; cardless chat proposals qualify only during a two-minute grace.
- v5 admin shared M3E furniture now follows the mockups: an unboxed page line
  (display title, bold mono count, serif context, no ⓘ), a "Live HH:MM" chip
  and "Ctrl K" pill, 48 px window title bars with pill search and pill
  controls, panes flush to their window with list items as card surfaces and
  a tonal selection, mono overlines, pill and connected buttons, tonal status
  chips, an icon Approve key with Reject taking the risk fill when Approve is
  blocked, no visible "open" on selected rows, and the phone drawer's Week
  count and Ctrl K hint. A hovered list row now gets its own state layer
  (`--row-hover`) instead of turning the pane's colour.
- v5 admin Fixed, History and Inbox follow their mockups more closely: a
  Fixed timing's whole row is the selectable card (one button per row) at
  the mockup's density (weekday over time, 28 px portraits, flags inline, the
  party as one line of names), its flags read in lower case and Add carries
  a plus; History's filters read
  "Week: every week" and the change pane drops its extra header row; Inbox
  section labels are mono overlines with tonal header chips, and an item
  opened on a phone has "‹ Inbox" in the top bar, a compact header and a
  bottom action bar (pencil, Reject…, Approve).
- v5 admin Fixed's boss picker shows each boss's difficulties on one line in
  compact rows (an existing timing lists its own bosses until "All n
  bosses…"), with 40 px fields and name-chip party picks; the Inbox item
  shows the boss art, the member's words as a speech bubble, participant
  changes as chips, the thread as rows with initials and used messages
  lifted (ready for a per-message `used` mark and a Used/All toggle), the
  message time opening Discord instead of an "open" link, and a decision
  pane that explains a blocked Approve and what Reject does.
- v5 admin Inbox threads come from the item's `thread` (the channel messages
  around the evidence, each marked used) with a Used/All toggle starting on
  Used; Fixed rows open from anywhere on the row without a positioned overlay
  (WebKit-safe) and keep their fill when hovered; selected Fixed and History
  rows also go bold; the phone's back step returns focus after the drawer.
- v5 admin History's Checkpoints tab says "No backups recorded yet" (no
  backup directory configured) instead of an empty table and hides the
  Timeline's filters; tabbed title bars centre their window dots on the tabs.
- v5 admin History rows carry the mockup's dot, a facts line with the row
  count and a one-line summary; a change's fields read one per line, and on
  wide screens "Revert a member's changes…" sits at the foot of the open
  change. Fixed timings lay out as grid rows (still one table, one button per
  row), with Home channel on its own line in the editor.
- v5 admin Inbox threads keep cited messages that were deleted (merged back
  in as used, "no longer stored") and start on All when none is used; the
  phone action bar is focused pencil → Reject… → Approve as it is seen; Fixed
  rows show the party as plain text (full list in the tooltip) and ring a
  focused row inside its card.
- v5 admin Fixed's editor follows B_Fixed's spacing: sections 12 px apart,
  "All n bosses…" and the typed bosses on one line under the rows, the note
  after the party, and the party showing its picks and the first few others
  with a "+n" chip for the rest; the list head lines up with the rows, and on
  phones the party sits under the bosses with no sideways scroll.
- v5 admin planner drops set the time as well as the day: a run dropped
  between runs starts right after the one above (its start + run length), at
  the top of a day it ends right before the run below, an empty day keeps its
  time and own-time runs stay untimed, held within 00:00–23:59. The drag shows
  and announces the resulting time ("→ 23:00"); an overlap that double-books
  a member shows a clash warning (icon and words) on the indicator and both
  cards but still saves. Keyboard moves step by the default run length;
  Shift+Up jumps to just after the previous run, Shift+Down to just before the
  next. A drop made while someone else changed the week is refused as a
  conflict rather than overwriting it. Config gains a Run lengths section
  (default minutes with a slider, boss + difficulty overrides checked against
  the boss list).
- v5 admin app shell (M3E slice 1, gates G1, G2, G6, G7): a navigation rail
  replaces the masthead's grouped nav (96 px, expanded to 240 px from 1440 px
  wide and collapsible, remembered per browser; Inbox badge; account at the
  foot), a 36 px page line replaces the page-head cards (title, count, page
  controls, an ⓘ for one-time help, the Live chip and Commands), windows get
  a 20 px frame and the 1180 px page cap is gone. Phones and phone landscape
  get a 48 px top bar (menu, title, Live, Inbox) and a navigation drawer that
  traps focus and returns it to the menu, instead of the pinned links and
  "More". New M3E tokens (`--select*`, `--pane`, `--board`, `--row`,
  `--chip-fill`, `--seg-fill`) with contrast overrides and a per-face check.
- v5 admin page line: the title group (title, count, context) sits in an
  outlined surface shape with 12 px corners, in every colourway and face (ink
  read washed out on the bare ground, e.g. blossom); the controls stay their
  own chips, History's filters become field chips, and the line stays 36 px.
  Chat's per-model stats no longer stack one row per model: compact chips for
  the two busiest models ("model 37 ✓ · p50 2.2 s") and a "+n models · e
  errors" button opening the full per-model table keep the line one row.

**Fixed**

- v5 admin Bosses: a whole catalog or event row selects its boss, weekly timings
  read as one outlined list and open that timing's editor in Fixed
  (`/fixed?open=<id>`), difficulty notes sit in their own accented callouts, and
  seasonal chips use a short `Seasonal boss · CW3` tag (full season name on hover)
  so the event list no longer scrolls sideways.
- v5 admin Inbox shows a proposed status change as status chips under its
  field name (old struck, new toned, e.g. At risk → Unconfirmed) instead of
  raw `at_risk`/`planned` text pushed into a side column.
- v5 extraction refuses hints naming only done or cancelled runs for kinds
  that act on existing runs; Add/Fix keep v4 behaviour. Dayless RSVP/Sub
  answers stay in their evidence message's boss week without escaping a
  channel that has live runs; bare clocks keep their implicit resolved day
  ("Refuse + anchor", including on rescans).
- v5 Discord chat: asking which seasonal bosses or guides exist now names the seasonal event bosses (Kai, Meilin) with their event and availability.
- v5 Discord chat: after a schedule lookup, a reply announcing a posted card
  (e.g. a move proposal waiting for ✅) is no longer replaced by the lookup's
  run listing; the card reply posts as the model wrote it.
- v5 admin History: the window no longer scrolls away when a row opens, only
  the clicked row of a multi-week change is marked open, tabs and Week/Who
  filters match the other M3E title bars, a change shows compact field diffs
  with the raw JSON in a viewer, and repeated summary lines collapse into one
  with a count. History, Fixed and Members no longer draw two sets of window dots.
- v5 re-reading party channels while watching is paused or the extractor is
  off now says so and points to Config → Watching (API `409 extraction_off`,
  same wording from `/rescan`), instead of "The service is unavailable".
- v5 admin Week help ("How to move runs") now opens as a contained card and
  mentions swapping; the run sheet keeps its action buttons on their own row so
  a moved run's extra actions no longer squeeze the run details.
- v5 long slash-command replies now post their follow-up parts in serve;
  the production transport refused every follow-up (and would have refused
  any other default-bodied transport call) as invalid without sending it.
- v5 chat queue fixes: waiting questions' keycap positions are renumbered
  when a question leaves the queue; a queued question whose channel left the
  chat category is refunded instead of answered; a panic while a question's
  context is built refunds its allowance and logs a failed row. A lookup
  that panics again while the question is being cleaned up no longer aborts
  the process: cleanup is deferred off the unwinding stack and, if it still
  fails, only settles and refunds.
- v5 extraction reads each message version once across live bursts, backlog
  drains and startup/manual rescans (in-memory claims; the other reader
  defers and gets the row back if the owner fails). Rows are marked read
  before proposals or cards are posted, so a crash never repeats effects; an
  edit during a running call drops the stale answer and the edit is proposed
  instead (v4 proposed the old text). Gateway receipt times use the injected
  clock.

## 1.0.0-beta.1 (2026-09-28)

**Added**

- v5 gives countdown and weekly digest headers a bounded persona-flavored
  interjection, preserving code-owned schedule facts and stable stored text
  across retries and edits; schema v18 stores phrases before delivery claims.
  Rolling back to a v17 image requires the paired pre-upgrade volume backup.
- v5 stores ordered Reply-profile assignments by Discord role and applies
  role-first styling live, with current-role validation and conflict-safe admin
  updates; Config now has a named guild-role picker and add, change, remove,
  reorder and conflict-recovery controls.
- An opt-in, governed synthetic privacy re-identification probe that keeps
  network calls out of tests and reports only aggregate recovery counts.
- Source-checked v5 compatibility inventory covering routes, commands, runtime
  entrypoints, configuration, storage, workers, chat tools, and portal assets,
  with regression checks for missing entrypoints and environment dispositions.
- Relocated the runnable v4 rollback implementation to `legacy/python/` without
  moving private deployment state.
- Added initial portable migration schema modules, synthetic history/delivery
  fixtures and independently validated JSON Schema artifacts; runtime export,
  import and maintenance operations are not implemented yet.
- Added deterministic, bounded migration archives, pure guild/catalog/reference
  preflight checks, and private no-overwrite publication with Linux/macOS path-race
  protection. These helpers do not yet export or import a running database.
- Added the modular Rust runtime foundation with explicit offline serving,
  loopback health checks, strict configuration, redacted errors and bounded
  shutdown. Scheduler, storage and Discord adapters remain under development.
- Added synthetic Python-derived domain and initial stateful scheduler vectors,
  with replay/drift checks and full input-schema validation before opening the
  oracle store. Broader scheduling and migration parity remain in development.
- Added an offline Rust LLM-provider interface, deterministic fake provider,
  structured output/tool-call validation, bounded payloads and retries, and
  redacted diagnostics. Kanata networking and chat integration remain deferred.
- v5: capability-aware model requests (Kanata `/v1/models` metadata, operator-declared
  capabilities or a minimal profile) and a generic OpenAI-compatible HTTP adapter for
  Kanata or a plain Ollama `/v1` endpoint: https with verified certificates, plain http
  only for local hosts, optional bearer key, a single capability lookup per call and one
  reshaped retry when a model rejects a field. Env/config wiring is still pending.
- v5: Rust domain core (reset and calendar weeks, fixed slots, time parsing, IDs, boss
  catalog validation) matching the frozen v4 domain vectors, including DST handling.
- v5: strict persona layout under `config/personas/` (catalog, bundles, profiles) with
  a Rust loader, trusted Kanade fallback, atomic reloads and per-turn snapshots; the
  Kanade bundle adds a header-rewrite prompt for small models. Public v4 persona prompt
  vectors, plus reminder and run-mutation scheduler vectors, are frozen for parity.
- v5: Rust scheduler core for week materialisation, fixed-run adoption, reminder rows,
  RSVP reactions and run lifecycle, behind a store interface with an in-memory store and
  a shared conformance suite. A day-of ping can no longer fire after its run starts on
  DST spring-forward days. Dispatch, mention and digest scheduler vectors are frozen.
- v5: run mutations (status, amend, swap, fixed-run edits) plus new fixed-edit choices
  per manually moved run ("update to fixed" or "keep for this week") and a "reset to
  fixed" action that refuses slots already passed. Removing someone from a fixed run
  drops their RSVPs and recomputes status. A pure notification core plans mentions,
  reminder dispatch and weekly digests; reminders and notices whose home channel is
  unavailable now fall back to the post channel with an admin warning instead of being
  silently dropped.
- v5: Discord adapter groundwork on Twilight: sends report delivered, definitely not
  sent or ambiguous (never retried), mentions limited to explicit member lists, v4
  gateway intents with clear fatal-close reasons, reaction RSVPs and roster events,
  and a slash-command framework with v4 permission gates and 3-second-safe replies.
- v5: SQLite store (SQLx, no TLS in the store) with checksummed migrations, atomic
  commits guarded by a store revision (conflicting saves are re-planned), single-writer
  ownership, and private (0600) online backups; restore validates the copy first and
  refuses to run beside leftover SQLite journal files.
- v5: store ownership hardening: an operator lock directory (`owner_lock_dir`, 0700)
  with a lock named after the database file's identity, private canonical paths with no
  symlinks, writes and backups refused if the database file is replaced, and
  cancellation-safe open and restore.
- v5: tamper-evident schedule history: every change records who, where and the
  before/after rows in a hash chain the database refuses to edit; admins can revert a
  change, restore a boss week to a point or undo a member's changes (always forward,
  never resending pings); repeated requests apply once; backups carry a history anchor.
- v5: history blame (who last changed each run or fixed-run detail) and immutable
  checkpoints (automatic at every boss-week start plus admin-named) with a previewed,
  week-scoped restore.
- v5: schedule drafts (git-style branches): admins stage changes that ping nobody,
  preview the resulting week, rebase and merge them as one recorded change with
  per-detail three-way conflict checks (no force); merges post one summary per affected
  channel, and drafts for a boss week that has passed expire automatically.
- v5: edits made from an out-of-date screen no longer silently overwrite someone
  else's change: each edit can state the last change it saw per detail, a clash on the
  same detail is refused with who changed it and when, different details still merge,
  and only admins can apply theirs anyway (recorded as an explicit override).
- v5: member requests (git-style pull requests): members ask for a new weekly run, a
  weekly-run change, or to join, leave or swap; admins preview, edit, approve (as one
  conflict-checked merge) or reject with a reason. Permission is re-checked at
  approval, party changes are stored as add/remove so concurrent requests merge,
  members have pending and daily limits, frozen members can't submit, and public
  channels only ever show a bot-written summary of the request.
- v5: cherry-pick: admins re-apply a past change (time, party, status or answers) to
  the same weekly run in another boss week, previewed first and re-checked against
  the schedule rules; conflicts refuse by default, and force applies exactly the
  conflicts that were previewed and is recorded as an override.
- v5: attendance model (off by default, v4 behaviour unchanged until enabled): members
  can set a standing "always in" answer per weekly run and admins can mark a weekly run
  "assume everyone's coming"; answers read confirmed / assumed / unknown / declined with
  tallies like "4/4 (2 assumed)", runs turn at risk only on a ❌ or on unknown answers
  close to start, morning pings mention only unknown members, and marking a run done
  records who came (members confirm only their own), with private attendance patterns
  that suggest standing answers. With attendance on, an admin-set planned/confirmed
  status sticks (shown "set by admin") until a reaction, party change, move or new
  status ends it, and a run's status no longer changes once it has started (after the
  start only a new status ends the pin).
- v5: model traffic governor: per-backend permit groups with priority queues (admin
  first, extraction and follow-ups last), a request-rate ceiling, a capped retry
  budget and a circuit breaker that sheds new work during an outage and ramps back
  up with a single probe. Model calls go through it: a chat question holds one slot for
  all its tool rounds with a request cap of rounds + 1, gateway admission refusals are
  requeued once after Retry-After, a 504 ends the question, and retries need budget.
  Nudge rewrites get their own session that never waits and sends one request, and a
  content-filter stop is reported apart from a length cut-off (logged as
  `content_blocked` by extraction).
- v5: model requests match the real Kanata gateway contract: its published per-model
  admission limits are read and checked against the limiter's permits, request values
  are validated locally (so a bad value never marks a model unsupported), Kanata's
  breaker wait is honoured, "off" reasoning sends `none` where supported, empty tool
  results get a placeholder, and every request carries an `x-request-id`.
- v5: persona prompt compiler matching v4's assembled prompts byte for byte, with strict
  staging lines (literal boss-name substitution), optional self-service nudge pools and a
  nudge rewrite prompt per bundle/profile (Kanade ships its own lines). Persona YAML no longer coerces unquoted
  numbers or booleans into text.
- v5: storage for extraction and chat logs (per-round chat detail, filterable by model,
  date, outcome, channel, member, tool and text with keyset paging), rescan jobs,
  allowance overrides and one self-service tip per member per week, plus extractor/chat
  proposals stored as system drafts that expire and supersede older ones. Logs are
  pruned after 90 days in small batches.
- v5: HTTP server with separate admin and public listeners (admin routes never exist on
  the public one), Host and proxy-header guards, the PWAs' security headers, body and
  time limits, and static serving of both apps and boss art; the public portal answers
  closed. `KANADE_BIND` is renamed `KANADE_ADMIN_BIND`.
- v5: models follow Kanata: each role's model and reasoning level come from settings and
  are checked against what Kanata publishes, a role counts as cloud when Kanata's trust
  zone says so (or is unknown), `KANADE_ALLOW_EXTERNAL_UNMASKED=1` lets such calls through
  for testing with loud warnings and flagged log rows, and `kanade models check [--probe]`
  lists Kanata's models and sends one tiny test request per role.
- v5: `kanade import v4` brings v4's weekly fixed runs and recent chat and extraction logs
  (with the messages they reference) into v5 from a read-only v4 snapshot, previewing by
  default and adding nothing when run again; the image now carries the boss catalogue.
- v5: `kanade serve` now runs live without Discord: it owns v5's own database, loads the
  catalogue, knowledge, personas and settings (env seeds under saved values), and serves
  the admin app against real data; sessions report how the admin signed in. Kanade chats
  in every channel of the configured chat categories (no per-channel chat list).
- v5: the model client is built at startup from the configured Kanata endpoint, key and
  per-role models; an unreachable model list leaves extraction and chat degraded rather
  than stopping the bot, and unpublished reasoning levels are reported.
- v5: schedule notices use v4's exact wording (moves, cancels, own time, done, restores,
  swaps, weekly-timing changes, quiet mode, "via portal"); approving Kanade's card only
  announces moves, as in v4; notices older than 6 h are dropped instead of posted late,
  an unreadable one no longer stalls reminders, and delivered notices are purged after
  90 days.
- v5: notices from schedule changes (admin edits, inbox and ✅ approvals, requests,
  rollbacks, expiry) are saved with the change itself and posted once by the reminder
  tick, surviving crashes without being lost or sent twice.
- v5: container packaging in `deploy/`: a hardened ~50 MB distroless image (non-root,
  read-only, no toolchains) and a Compose stack that serves the admin portal on the
  tailnet through the existing edge site, with secrets from host files and a runbook
  for switching between v4 and v5.
- v5: the Discord adapter now reads messages, channels and threads (a thread counts as its
  parent channel), keeps a guild cache for channel names, reachability and the watched
  categories, drops every event from other guilds or DMs, and can reply, page members,
  page channel history and list channels. Commands are only ever registered per guild.
- v5: runtime settings stored in the database (v4's `config` keys where they exist, new
  `v5.` keys for channels, categories, reset and modes), seeded from env until saved, and
  saved one section at a time.
- v5: serve configuration for the live bot: Discord token from a file only (plain env
  refused), a required "v4 stopped" confirmation, guild/role ids, store paths, model
  endpoint and settings seeds, all validated at startup with secrets never logged.
- v5: startup loaders for the boss catalogue (tracked `boss/bosses.yaml`, parsed strictly),
  the boss knowledge directory (schema-checked) and the persona directory.
- v5: admin logs and rescan API: chat and extraction logs with filters, totals and facets
  (withheld questions stay hidden and unsearchable; extraction details list refused
  changes), and rescans of watched channels that start at once, report unread messages
  and can be cancelled safely. Failed extraction calls never show store error text, a
  cancel racing a finishing rescan always reports how it really ended, and log lists no
  longer load prompts.
- v5: admin inbox API: Kanade's extraction and chat proposals and members' requests in
  one list with a preview of what approving would change; approve or reject each
  (safe to retry), approve a proposed move, add or split at a corrected time in one step,
  and proposal decisions require a Discord sign-in. Discord card refreshes and notices
  follow once the bot is wired up.
- v5: admin history API: paged change history with a total and week/person filters,
  single change records, who-changed-what per run, a history integrity check, and
  revert, restore-week and revert-a-person's-changes with a preview that never writes
  and retry-safe apply. Cherry-pick and the history graph follow once their contracts
  are settled. Out-of-range change numbers get "not found"/"invalid" rather than a
  server error, a refused rollback lists every requested change, and an applied
  rollback reports the rows it actually saved.
- v5: admin edit API: move runs, change status, answers and parties, reset to the
  weekly timing, preview pings, create, edit and retire weekly timings, and edit
  members and aliases. Edits made from an out-of-date screen are refused (409) instead
  of overwriting someone else's change, a retried request is applied only once, and
  Discord member updates no longer overwrite portal member edits. Weekly-timing edits
  must send the week `version` they were loaded at (the admin app change follows),
  moves stay inside the run's boss week, and portal answers are recorded like v4 chat
  answers (any answer can be cleared; a hand-set status is kept). Not yet wired into
  `serve`.
- v5: admin read API: week, stats, summary, weekly timings, reminders, members,
  channels, personas, bosses, boss events and knowledge, each matching the published
  JSON schemas; members gain stored aliases, reply style and roles (migration 0010) so
  the staff check survives a restart. A correct break-glass token can no longer be
  locked out by guesses from many addresses, and an edge request without a usable
  `X-Forwarded-For` is refused. Discord member updates now carry roles and a computed
  Administrator permission, so admin sign-ins end as soon as someone loses staff status.
  Not yet wired into `serve`.
- v5: admin sign-in: Discord login (identify only, PKCE, one-time state, token revoked
  after use), Tailscale identity from the edge for allow-listed logins, and a break-glass
  token read from a file; server-side sessions with idle/absolute timeouts, a staff
  re-check every 5 minutes and CSRF checks on every change. Not yet wired into
  `serve` (it answers "sign-in unavailable" until the store and bot member cache are).
  Hardened after review: rate limits on sign-in, only `identify` scope and no bot
  accounts accepted, the client secret sent with HTTP Basic, an edge secret required
  before Tailscale identity is trusted (listeners may bind a private address by
  opt-in), and a same-origin landing page after Discord sign-in.
- v5: extractor and chat proposals: each amendment becomes a proposal only if it can
  apply now (otherwise it's refused with v4's reason and no card), is approved with ✅ by
  a run participant, an admin or the timing's owner, merges through the shared schedule
  write path, replaces older proposals for the same target, and expires silently after
  24 hours. Proposed answers recount the run's status like v4; approved cancels win over
  status changes since the proposal, and an approved move revives a cancelled run.
- v5: extraction proposal cards on Discord, formatted as in v4: posted through the
  delivery journal with no pings, ✅ to apply and ❌ to reject (by a participant, an
  admin or the owner), and refreshed when applied, rejected or superseded. Card details
  are stored with each proposal (migration 0011) so cards re-render after a restart;
  extraction logs record refused changes in a structured field. A post that never went
  out gives the weekly nudge tip back. A ✅ on a run edited by hand after the card went
  up is refused in plain words and the card is marked out of date; reactions from
  people who may not answer change nothing, and internal errors go to admins, never
  the channel.
- v5: self-service redirect decisions (move it yourself, request it, or keep the card;
  cards only while the public portal is closed) and persona nudges: rotating seed lines
  from the profile, bundle or built-in pools, at most one tip per member per boss week,
  with an optional 2-second rewrite on a small model that falls back to the seed line.
  Rewrites are refused (seed line used) for markdown, hidden format characters, invite
  links, changed placeholders or a word on a built-in safe-for-work deny-list (including
  sound-alike spellings such as "dih"/"bih" and common Malay, Indonesian, Singlish,
  Tagalog, Thai and Vietnamese swears), and the
  filled line is checked again. Kanade has her own rewrite voice and request lines.
  Extraction now plans the redirect for each kept change (still cards only while the
  public portal is closed) and uses up the weekly tip only when a link is posted.
- v5: chat traffic and safety: per-member allowance overrides from the admin store,
  refunds when a question fails through no fault of the asker, a short per-channel
  queue instead of dropping questions, a guard that pauses clean retries during a storm,
  a fixed reply when the content filter blocks an answer (personas may set their own
  `failures.content_blocked` line), and blocked questions kept out of everyone's later
  context. Not yet wired to Discord.
- v5: chat question loop and context: history, reply chains and anchors trimmed to a
  token budget, one limiter slot per question with a round cap plus one clean retry
  (also used when the content filter stops a reply), a failed card post told to the
  model, and one chat log row per question with a round per request. Like v4, the last
  round is sent without tools, and a call to an unknown or unoffered tool or with bad
  arguments gets a steering note instead of failing the reply. Not yet wired to Discord.
- v5: chat gate, authority, read and proposal tools (proposals only through the
  scheduler, refused up front when unworkable), reply sanitising and injection guards,
  matching the frozen v4 chat vectors; tools load in bundles as needed instead of all
  12 every round, and every tool argument and result goes through the identity codec.
  Compact times after a day (`wed 930`) read as evening like v4, and a bare hour after a
  weekday (`sat 10`) keeps the weekday instead of v4's day-of-month misreading.
- v5: extraction pipeline: watched messages are debounced into bursts, read through the
  model limiter (one answer retry), logged once per call with their outcome, and turned
  into proposals only through the scheduler; a bounded backlog drains slowly and waits
  out an open breaker, and rescans run one at a time and can be cancelled. Not yet wired
  to Discord (cards and reactions come next). An edit made while a message is being read
  is read again rather than lost, a misconfigured model route fails instead of retrying
  forever, and rescans retry only the turned-away pieces and report what stayed unread.
- v5: extraction rules ported (keyword gate, message bursts and rescan windows, day/time
  resolution, run matching, per-run merge), matching the frozen v4 vectors exactly,
  plus the extractor's answer schema, prompt, one-retry answer handling and burst
  planning. Member identities in extraction prompts go through the identity codec; the
  extractor checks model answers itself (v4 coercions) with one answer retry per call.
- v5: delivery journal for Discord sends (claim, bind to the channel actually used,
  mark ambiguous, retire rejected or replaced cards, recover in-flight sends after a
  restart without resending), a message-to-run card index for reactions, and a guard
  that never reopens a reminder whose delivery may already have happened.
- v5: scheduler tick and delivery executor: reminders and weekly digests go out
  through the journal in v4's order, never twice; sends that never left are retried,
  refused ones retire with a throttled admin alert, a digest is replaced only after
  the old card is confirmed deleted, and a clock that moved backwards posts nothing.
- v5: the admin app's history follows the Rust history API: plain labels for who changed
  each run field, reminder changes described (a move's re-placed reminders fold into one
  line), guild-local week headings, and a revert dialog that lists conflicts and previews
  what forcing would change. Reverts no longer send a conflicting request id.
- v5 admin app: no more flash on background refresh (the Answers chart is updated in place);
  tool-trace arguments and results open in a dialog; Copy transcript (Markdown or JSON) on
  chat turns; Reminders and Reply profiles gain paging, filters and one-line rows; Maple Mono
  is the code font.
- v5 extraction stops at once when switched off or paused (in-flight calls cut, rescans
  cancelled), restarts only read unprocessed messages (no reposted cards), chat questions are
  never extracted automatically, and only regular messages and replies are read.
- v5 admin app brand shows the live Discord bot: its name (server nickname, global name, then
  user name) and its avatar and banner, cached from Discord's CDN into `/data/identity` after
  each READY or profile change, with versioned URLs so browsers pick up new art.
- v5 container logs explain persona and chat behaviour: `persona_selected` at startup (fallbacks
  and profile load problems warn), `settings_changed` with keys and before/after values,
  `persona_switched`/`personas_reloaded`, and chat lifecycle events (`chat_admitted`,
  `chat_ignored` for summons only, `chat_answered`/`chat_failed` with persona, profile, model
  and route, `chat_cancelled`, `chat_setup_changed`), never with message text or member ids.
- v5 serve runs extraction live (S9): watched messages feed the pipeline, proposal cards post
  and resolve with ✅/❌ (one card desk for extraction and chat cards), `/rescan` and the admin
  Rescan panel page Discord history, a 24 h startup rescan runs when extraction is on, chat
  questions are never extracted, health reports `extraction`, and shutdown cuts in-flight
  extraction calls so it stays inside the container's stop grace.
- v5 admin app: role and bot mentions resolve to names, message authors are copyable names,
  Config → Models shows read-only capacity groups with one line for Kanata's limits, the
  masthead is regrouped (status chip, account menu, compact Commands), state pills share
  semantic colour profiles, the chatbot rate fields sit on a grid, and the model picker lists
  base models only.
- v5 chat answers mentions of the bot's managed role, drops queued questions when chat is
  switched off, never calls the model for a question deleted before its answer starts, keeps
  unposted answers out of history, survives a panicking answer, and bounds chat shutdown.
- v5 chat allowance may be 0 per member ("staff only"): members without an override are ignored
  silently, staff are exempt, and `/limits` says chat is staff only.
- v5 chat logs keep each tool call's result (up to 8 KiB, marked when cut) and its time; the
  turn view also shows imported v4 outputs and timings. `kanade import v4 --refresh-logs`
  re-imports only `v4-` log rows, and v4 rounds with no requested tools take the call names.
- v5 serve runs the Kanade chatbot in every channel and thread of the chat categories: pilot-role
  gate, queue-position reactions, allowances with refunds, replies that ping nobody, chat log
  rows, proposal cards approved with ✅, live on/off from settings, graceful shutdown, and a
  `chat` state in health.
- v5 config API reports the capacity groups the bot actually runs (default `gateway` group or
  `kanade.toml` groups, with `groups_source`), checks each group against Kanata's limits, and
  no longer presents the bot's permits as the key's limit (`key_limits.max_in_flight` is null).
- v5 admin app shows display names instead of raw Discord ids (click to copy the id),
  reply profiles as a table, and a tidier Chat log: the implicit bot mention is dropped from
  question previews, times are guild-local ("Mon 28 Sep · 00:00"), and model/duration/name
  cells stay on one line.
- The private v4 rollback env moved from the root `.env` to `legacy/python/.env`; the v4
  Compose file and guides point there.
- v5 reads non-secret settings from `kanade.toml` (`KANADE_CONFIG`; template
  `kanade.example.toml`), including model capacity groups; `KANADE_*` env vars still work
  and override the file, and unknown keys stop startup naming the key only.
- v5: slash-command replies longer than Discord's limits are split at line boundaries
  into follow-up messages (`/fixed list` no longer hangs on "thinking…"); a refused
  completion gets one short fallback edit, and the interaction log names the failure kind.
- v5: the admin API sends name data so the app never needs raw ids: a roles list
  (`/api/admin/roles`), the bot's own user id in the identity, author ids on evidence and
  extraction messages, and full member ids on chat rows.
- v5: serve logs its model setup at startup (listing, each role's model, effective
  reasoning level and its source, refused or unmasked routes), and
  `KANADE_{EXTRACT,CHAT,REWRITE}_REASONING` seed the reasoning levels so
  `kanade models check` matches what serve would use.
- v5: Kanata's `model:level` aliases (e.g. `gpt-6-luna:high`) are recognised as reasoning
  variants of their base model: the config API marks them, a role saved on one uses its
  fixed level, and `kanade models check` lists them under the base.
- v5: reasoning "off" is only used where Kanata allows it (no published list, or one
  that includes `none`); models that require reasoning get their lowest published level,
  the Config API refuses "off" for them, and `kanade models check` shows the level each
  role actually uses.
- v5: live Discord wiring in `serve`: connects with the bot token only when
  `KANADE_EXPECT_V4_STOPPED=1`, registers the retained commands for the guild only,
  syncs the roster (so admins can sign in with Discord), runs the reminder and notice
  tick, handles card and RSVP reactions, reports Discord and scheduler health, and shuts
  down in order; `KANADE_DISCORD_GATEWAY=0` runs the admin API alone. Commands from
  Discord (including `/swap`) no longer carry "(via portal)", and a Discord `/fixed edit`
  posts nothing, as in v4. Interrupted sends are recovered before Discord starts, and a
  refused login (bad token or intents) leaves v5 up but degraded instead of restarting
  and logging in again with the shared token.
- v5: admin config API: the Config page reads every runtime setting, Kanata's live model
  list and the server-only facts, saves one section at a time (reasoning and capacity
  rules checked, safe to retry), and reloads persona profiles live.
- v5: slash commands ported from v4 (guild-scoped): `/fixed add|edit|remove|list`,
  `/schedule`, `/amend`, `/status` (now also covers cancel, own time, done and restore),
  `/swap`, `/rsvp`, `/pings`, `/style`, `/limits`, `/rescan`, `/say`, staff-only `/nick`,
  `/debug ping|clear_test`, with v4's permissions and wording; `/bot`, `/pingtime`,
  `/debug status` and `/debug extract` are dropped in favour of the admin app.
- v5: boss names and difficulty pills no longer run together: Reminders lists one boss per
  line and inline boss tags keep a gap between bosses.
- v5: design experiments (on by default; `?experiments=off` or the command palette turns
  them off): a small morphing loading indicator on buttons while they save, sign in,
  reload or decide, and a gently waving progress bar on rescans; both stay still under
  reduced motion.
- v5: the week planner's cards are wide again (v4's 230px day columns) and the drag grip
  has its own column, so it never covers a boss name or difficulty badge; the Limits page
  says it isn't available yet instead of loading forever.
- v5: the admin app signs in for real: it offers only the methods the server enables
  (Discord, admin token), explains each Discord refusal, returns you to the page you
  were on after a sign-out, signs out properly, hides proposal decisions for non-Discord
  sessions, and says "not available on this server yet" for unbuilt routes instead of
  "Can't reach Kanade".
- v5: the admin inbox follows the Rust inbox: member request types and chat proposals are
  labelled, conflicts can only be rejected, "Move & approve" works for moves, new runs and
  splits, reasons are asked only for member requests, and token/Tailscale sessions are
  told to sign in with Discord to decide Kanade's proposals.
- v5: weekly-timing edits can no longer pair a fresh week version with an old row (which
  could silently undo another admin's change); only a real out-of-date refusal asks to
  reopen the form, and history names admins by who they are.
- v5: the admin app now sends the CSRF token and a retry key with every save, and the
  week version with weekly-timing edits (an out-of-date form shows the reload message
  and keeps what was typed); the dev mock enforces the same rules.
- v5: Svelte 5 web workspace (`web/`) with separate admin and public installable apps
  that keep the portal's look (with WCAG AA contrast fixes), run under a strict CSP with
  no inline code, work offline, and include a keyboard-accessible drag-and-drop planner.
- v5 admin app: v4-style masthead and grouped navigation with boss portraits and entry
  art, plus Week, run sheet (incl. Reset to fixed), Fixed (with per-run update/keep
  choice), Bosses with knowledge, Members and Reminders screens; a `web` CI job runs
  lint, type checks, unit and browser tests against the dev mock (`tools/pwa-mock`).
- v5 admin app: Inbox, Extractions (with re-read jobs), Chat, Limits (model backend
  groups, queue, breaker, admission refusals), History (timeline, diffs, previewed revert,
  restore-week and revert-by-member, checkpoints, "who changed this"), and Bosses pages
  on the new knowledge files; pages load on demand and each app ships its own styles.
- v5 admin app: full Config settings (12 sections incl. three-role model picker,
  per-alias capacity check against Kanata's published limits, reasoning levels
  limited to what each model publishes, a fail-closed cloud/PII warning, the
  self-service redirect mode, public portal toggle, reply-style profiles shown
  read-only with reload, publish and role order), a Channel access section, per-run
  re-read buttons, and a lazily-loaded planner (admin initial JS 71.8 → 43.6 KB);
  settings saves refuse read-only or unknown fields; proposed PWA contracts in
  `docs/v5/admin-api.md` and `docs/v5/limits-contract.md` for the backend to confirm.
- v5 planner cards: the drag-handle column is gone; a small grip sits in the corner,
  the whole card drags (after a short move, or a long press on touch), `M` picks a
  card up from the keyboard, and a press made before the drag code loads still drags.
- v5 admin app: Inbox works by keyboard on phones (arrows browse, Enter opens, Back
  returns to the item), empty tabs explain themselves, the week header fits one row
  at laptop widths, and Chat/Extractions filters show only valid chips and no stale
  rows after an error.
- v5 public app: while the portal is closed only the app shell, status and bot
  identity are served, and the page keeps checking so a reopened portal appears.
- v5: `boss/knowledge/` schema v2 with per-difficulty facts (levels, force, HP, spec)
  and credited iSIingGunz guide imports (Jupiter, Radiant Malefic Star, First Adversary,
  Baldrix, Limbo, seasonal Kai), plus `scripts/boss_knowledge` fetch/validate tooling
  with an anti-copy guard.
- v4 tooling: `scripts/convert_personas_v5.py` converts a v4 persona directory
  (`personas.yaml`, persona folders, behaviours) into the v5 catalog/bundle/profile
  layout using the v4 loaders, with a names-only dry run, no overwrite without
  `--force` and private file permissions.
- v4 rollback Compose now mounts the private v4 persona files from
  `config/personas-v4/`, leaving root `config/personas/` for the v5 layout.
- Added the unreleased Python maintenance foundation with v14 state/lease tables,
  exclusive store ownership, pre-upgrade snapshots and task-bound retirement.
  Startup recovery authority is revoked before repository access is returned.
  Operational maintenance, transfer and production upgrades remain disabled or
  unapproved pending delivery journaling, ingress and reconciliation integration.
- Added v15 per-source adoption-resolution storage with protected control-plane
  writes and snapshot-first synthetic v13/v14 upgrades. Existing upgrades stay
  BLOCKED/pending; runtime classification and private/live database upgrades remain
  out of scope.
- Added offline v13/v14 first-adoption source classification with content-free,
  domain-separated evidence and a fixed-purpose task-bound BLOCKED seed lease.
  Adoption remains pending; no Discord fetch, fake send attempt, or memory-family
  classification is introduced.
- Added private synthetic whole-message adoption binding and reasoned source/
  attempt retirement under a separate persisted BLOCKED task authority. Synthetic
  observations are not proof of remote authorship; no fetch, release, or memory
  handling is included. Only reminder and card message groups may bind or retire
  multiple targets; multi-target decline or digest groups are rejected unchanged.
- Added the J0 delivery-journal kernel: immutable send plans, request/observable
  fingerprints, pre-send intent and target claims, one-call transport handling,
  and atomic native bindings. The kernel alone was not a release or
  live-operation authorization.
- Added J1 v4 Discord effect wiring for reminders, digests, grouped extractor
  cards, and slash/portal debug cards. Bound effects precede reactions; J1 card
  edits, cleanup deletions, and RSVP reactions fail closed under maintenance
  leases. Ambiguous sends are not retried, and confirmed digest/debug deletions
  retire exact bound targets with durable reasons. No production use or release
  is authorized.
- Added J2 v4 chatbot source/semantic-slot journaling for final, refusal, staging,
  and rejection-follow-up replies. Chat placeholder edits, deletion, and reactions
  are lease-gated; an uncertain edit does not trigger a second final reply.
- Added J3 journaling for non-memory mutation notices, repeatable `/say` and guide
  posts, and decline notice binding/retraction. Guide fingerprints cover validated
  decoded files and ordered embeds; uncertain sends are not reported as refusals.
  Personal-memory effects remain excluded, and no release is authorized.
- Added G1 closed-state runtime attempt recovery: a content-free blocker report,
  operator bind of intent/indeterminate attempts to verified bot-authored message
  evidence, and reasoned no-replay retirement (reminder skipped shape, decline
  cooldown, digest week marker, retained v15 memory as `feature_removed`), all under
  a persisted task-bound BLOCKED/FROZEN-only lease. A retired card leaves its
  amendment proposed and reported; extraction never reposts it, so it is resolved
  by portal approve/reject. A retired reminder is never re-armed by a ping-time
  change; moving its run still schedules fresh reminders for the new time. A
  retired decline permanently suppresses further decline notices for that run and
  member. Binding a current-week digest records the week so the reset tick does not
  replace it; a future-week digest retires without a marker, so that week's reset
  still posts (an earlier preview that did land may need manual deletion). Bounded
  Discord history listing is advisory only. No sends, endpoints, or BLOCKED→OPEN
  transition.
- v4: Config → Models picks the extraction and chat Kanata aliases and reasoning
  levels at runtime from the live `/v1/models` list (also `GET /api/config/models`
  and `bossctl config set`), with capability badges, reasoning levels limited to
  each alias's published list, and a privacy warning for `external` or `-cloud`
  aliases. Selections are audited, stored as runtime config rows seeded once from
  `EXTRACT_MODEL`/`CHAT_PILOT_MODEL`/`EXTRACT_REASONING`/`CHAT_PILOT_THINK`, carried
  in bundle `runtime_config`, and honored by startup checks.
- v5: production pseudonym codec (`PseudonymCodec`) that replaces member names,
  aliases, mentions and ids with per-session random fictional given names and
  decodes model output back; not yet wired into chat or extraction (off by default).
- v5: provider-boundary leak scanner that refuses a masked model request still
  carrying a member name, id or Discord snowflake, before anything is sent.
- v5: reminders and the weekly digest post as v4-style embed cards with boss
  portrait and day-of entry artwork uploaded as attachments; the day-of heading is
  rewritten in the persona's voice by the small rewrite model (v4 text as fallback),
  and cards refresh after ✅/❌ reactions.
- v5: pseudonymization switch `models.pseudonymize` (`KANADE_PSEUDONYMIZE`, off by
  default) masks member identities in chat, extraction and rewrite prompts, fails
  closed without a roster, logs `external_masked` routes and `identity_leak_blocked`
  refusals, and stores each masked chat turn's model view and name mapping for admins.
- v5: chat turn details record the persona, reply profile, per-round model, effort
  and route as actually sent, errors and tool rounds, and show admins a "Model view
  (masked)" section with the masked rounds and fake-name table; extractions can be
  filtered by `identity_leak`.
- v5: model and reasoning changes saved in Config apply from the next chat question,
  extraction call and rewrite without a restart (new aliases stay fail-closed until
  Kanata lists them; ungrouped aliases are refused); rewrites now send the rewrite
  role's reasoning level.
- v5: temporary 64k context window for chat and extraction.
- v5: `/debug ping` posts a `🧪 TEST — ` copy of a run's day-of, countdown, amend
  or decline message (embed cards with art) whose ✅/❌ drive real RSVPs, and
  `/debug clear_test` deletes the channel's recent test cards.
- v5: masking now also covers single words of multi-word member names (e.g. a
  first name inside party channel names); words shared by two members get a
  neutral fake name that decodes to nobody.
- v5: chat schedule answers show only the runs the model named (named difference
  D-GROUND-FILTERED) and keep the rest of the answer, including code; replies too
  long for one message continue in follow-up messages instead of being cut.

**Changed**

- v4: Kanata request bodies follow per-alias capability metadata from `/v1/models`
  (cached, minimal when absent): `response_format`, `temperature`/`seed` and
  `reasoning_effort` are sent only to aliases that accept them, so extraction and
  chat work on cloud and codex aliases. A 400 naming one of those fields drops it
  for that alias (for the cache TTL) and retries once. The extraction prompt budget
  counts the in-prompt JSON schema, and the client trims a trailing `/v1`.
- Changed the new Rust release and supporting spike code to GPL-3.0-only;
  the independently licensed Python rollback tree retains MIT.
- Organized the Rust bootstrap into responsibility-based runtime, API and CLI
  subdirectories, with a dedicated integration-test directory.
- Refined the unpublished migration contract to preserve grouped Discord sends,
  require matching proposal-card bindings and canonical Discord IDs, and reject
  excluded memory/generic delivery records.
- Restored SQLite foreign-key enforcement when reopening existing v14 repositories,
  including a FROZEN maintenance restart.
- Schema v16: opening a v9–v15 store takes the pre-upgrade snapshot, then drops
  every `chat_memor*` table; retained delivery-journal rows are kept, and a v15
  store keeps its maintenance state unchanged. The snapshot still contains the
  old memory rows. Leftover `CHAT_MEMORY_*` settings are ignored with one startup
  warning.
- v4 extraction and chat now call the Kanata gateway's OpenAI-compatible API over
  verified HTTPS with a bearer key read from `KANATA_API_KEY_FILE` (a Compose secret
  in the container), replacing the direct Ollama client and the `ollama` package.
  New settings: `KANATA_BASE_URL`, `KANATA_TIMEOUT`, `EXTRACT_MODEL`,
  `EXTRACT_REASONING`, `MODEL_CONTEXT_TOKENS`; `CHAT_PILOT_MODEL` and
  `EXTRACT_MODEL` have no default and are required at startup when their feature is
  enabled; a database extractor switch that outranks `EXTRACT_ENABLED=false` gets
  one startup ERROR instead of per-burst warnings. Extraction uses a strict JSON
  schema (closed objects, all fields required). A trailing `/v1` on
  `KANATA_BASE_URL` is trimmed. `OLLAMA_*` settings are ignored with one startup
  warning naming each replacement; the `ollama` pytest marker is now `live_model`.
- v4 `.env.example` now lists only the settings an operator must fill in; every
  other setting, with its default, is in the `docs/setup.md` settings reference.
  Fresh databases now seed a 01:00 day-of ping and a single 60-minute countdown
  (was 09:00 and 60,15); existing portal-stored values are unchanged. Portal docs
  now describe the shared edge front door instead of the kanade Caddy service.
- v4 container files moved to `legacy/python/deploy/` (`Dockerfile`,
  `Dockerfile.dockerignore`, `compose.yaml`); build with
  `docker build -f deploy/Dockerfile .` from `legacy/python` and run Compose with
  `-f deploy/compose.yaml`. v5 container files will live in the root `deploy/`.
  The obsolete kanade Caddy image and Caddyfile example were removed; TLS ingress
  is the shared edge.

**Removed**

- Removed model-request pseudonymization and its external-unmasked opt-in for
  chat, extraction and rewrite. Configured external models receive raw member
  data with a warning; retired privacy keys now refuse startup. Historical
  masked chat records remain readable to admins under their existing retention.
- Removed the v4 personal-memory feature: typed-preference requests and prompt
  injection, `/memory` commands, enrollment notice DMs, proposal cards, the
  cleanup worker, the portal Memory pages, `/api/memory` routes,
  `bossctl memory`, and the `CHAT_MEMORY_ENABLED` setting.

**Fixed**

- v5 self-schedule lookup now recognizes an exact bot name copied into a
  model's participant argument without treating other members as self.
- v5 day-of reminder fields now label only members still waiting for an answer
  beneath the unconfirmed tally in either attendance mode; the top line retains
  the full party. V4_COMPAT scheduling, tallies and pings are unchanged.
- v5 resolves an explicitly first-person schedule question to its asker when
  a model omits the participant or invents an unrecognized mention;
  third-person requests remain distinct.
- v5 pauses new delivery admissions during gateway outages and until a fresh
  roster reconciliation succeeds, while allowing admitted work to settle.
  Cancelling an admitted send releases its in-memory owner without retrying a
  possibly committed journal claim or an ambiguous Discord operation.
- v5 masks complete links—including unknown handles in URLs—before model calls
  and refuses any URL left at the provider boundary; member-facing links are
  restored locally without storing the token-to-link mapping.
- v5 Reply profiles default to private until explicitly published; admins get
  per-profile and selected-batch Publish/Make private controls without stale
  saves republishing someone else's private choice, and member `/style`
  choices update without a restart.
- v5 admin Chat log Name copy buttons meet minimum pointer target sizes on
  desktop and phones without widening the dense table; the full-page axe
  check now includes the Who column.
- v5 waits for the initial roster reconciliation before its first delivery
  tick, and bounds Discord command-task draining on shutdown so stalled
  interactions cannot hold the store past the grace period.
- v5 Discord sign-in consumes a one-time callback state even when that callback
  is rate-limited, preventing it from being replayed after the quota resets.
- v5 chat replies keep code-block indentation and persona ellipses (`Mou...`
  no longer becomes `Mou..`).
- v5 admin: tool-trace buttons announce a short preview, selecting text in a
  viewer no longer closes it, pagers stay in range after a reload, and copied
  Markdown transcripts fence the question and reply.
- v5 admin Chat log: long questions are clamped to two lines (full text in the
  tooltip and on the turn page) instead of stretching the row.
- v5 admin Config → Channel access now shows the bot's permissions per watched
  and digest channel instead of "not available on this server yet".
- v5 public app: boss entry art on the week board rendered as full-size images
  covering the cards (the run-card styles were missing from the public stylesheet);
  a browser test now checks the art stays inside its card on both apps.
- A failed or maintenance-denied move/problem notice no longer aborts applying
  the remaining amendments on a ✅ card; digest and test-ping failures now say
  delivery was not confirmed instead of claiming Discord rejected them.
- v4 now starts on a BLOCKED, FROZEN or adoption-pending store instead of dying on
  a denied config seed: seeding runs only under an admitted OPEN operation, the API
  starts after Discord login, and the tick, heartbeat, on-ready
  roster/week/identity/backfill work and rescan worker are skipped with no database
  writes after bootstrap normalization (an adoption-pending store is still
  normalized to BLOCKED on open; reconciliation is flagged in-process). The healthcheck reads the mode and accepts a live API
  without a heartbeat while closed.

## v4 (Python, frozen rollback)

Releases 4.9.0 and earlier are in `legacy/python/CHANGELOG.md` (local only;
in git history up to `487c4ed`).
