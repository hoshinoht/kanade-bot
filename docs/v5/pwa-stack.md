# PWA stack decision: Svelte 5 admin + public

Status: adopted, 2026-09-24. `web/` is the production frontend; parity with
the v4 portal is tracked in `pwa-parity.md`, the visual language in
`pwa-design-guidelines.md` and the token/component map in
`web/packages/tokens/README.md`. `tools/pwa-mock/` is the dev-only mock server
the suite runs against; it is retired once the Rust binary serves the apps.

Decision: Svelte 5 + TypeScript + Vite 8 (Rolldown), no SvelteKit;
vite-plugin-pwa (injectManifest); bun as package manager and runner; two
separately built, separately installable PWAs (admin, public) sharing only
tokens, UI components, API types and a fetch client. The public bundle must
contain no admin code. Evaluated against the strict CSP below with zero
enforced and zero Trusted Types report-only violations; the contrast
corrections found on the way were approved.

## Layout

| Piece | Contents |
|---|---|
| `web/packages/tokens` | v4 tokens ported value-for-value, a separate contrast layer, self-hosted fonts (`@fontsource/*` 5.3.0, OFL-1.1, latin, v4 weights) |
| `web/packages/ui` | Window/tabs/modal/palette/toast/theme picker/board/table/chips/icons, nap illustration (moved from `web/shared/nap`, now removed), SW registration, `themeBoot()` Vite plugin |
| `web/packages/api-types` | Hand-written types for the mock JSON (`PublicWeek`/`PublicRun` vs admin `Week`/`Run`); to be replaced by ts-rs/specta output from the Rust API models |
| `web/packages/client` | Same-origin fetch client (no-store, timeout, typed errors) and bounded poller (no overlap, exponential backoff to a ceiling, stop after N failures, pause while hidden; `stop()` retires the in-flight run so a quick restart never sticks) |
| `web/apps/public` | Read-only week board + list + appearance; `offline.html` fallback page |
| `web/apps/admin` | History-API routed portal: Week planner (pointer drag via `@dnd-kit/dom` 0.5.0 loaded after first paint, keyboard move independent of it), run sheet, Fixed, Bosses, Members, Reminders, Config (theme), login window, pending windows for unbuilt pages, Ctrl/Cmd-K palette, undo toasts, uPlot 1.6.32 chart (lazy chunk) |
| `tools/pwa-mock` | Dev-only: admin on `127.0.0.1:4173`, public on `:4174` by default (e2e runs its own on `:4373/:4374`, real-art captures `:4383/:4384`); `/__mock/whoami` reports the pinned clock; production CSP + report-only Trusted Types; `/csp-report` sink; synthetic guild with fake names; clock pinnable with `KANADE_MOCK_NOW`; public origin has no admin routes; see its README |

Stack: Svelte 5.57.1, Vite 8.3.0 (Rolldown 1.2.10), vite-plugin-svelte 7.3.1,
vite-plugin-pwa 1.3.0 (injectManifest, workbox-precaching 7.4.1), TypeScript
6.0.3 (svelte-check 4.7.6 does not accept TS 7), vitest 5.0.1, Playwright
1.63.0 on installed Chrome (`channel: 'chrome'`), @axe-core/playwright 4.13.0.

## Baseline measurements (at adoption)

Bundle sizes (`bun scripts/measure.ts`, gzip -9; fonts raw woff2), taken when
the stack was chosen; current figures are under "History":

| | public | admin |
|---|---|---|
| Initial JS | 27.2 KB (Svelte runtime + ui + app + 0.7 KB theme boot) | 64.2 KB (adds dnd-kit, palette, planner) |
| Initial CSS | 7.5 KB | 7.8 KB |
| Total JS (incl. 4.8 KB `sw.js`) | 32.5 KB | 91.7 KB (uPlot chunk 23.5 KB loads with the Answers tab) |
| Total CSS | 7.7 KB | 8.5 KB |
| Fonts | 7 of 8 woff2 fetched on first view, 130 KB; 148.7 KB total | same |
| Precache | 26 entries, 280 KiB | 23 entries, 454 KiB |

The emitted `.woff` fallbacks are never fetched or precached. The mock server
does not compress; production must serve br/gzip.

| Check | Result |
|---|---|
| Public-bundle admin leak | Two layers. Build: `forbidModules()` (a `generateBundle` hook in `@kanade/ui/vite`) fails the public build if any module id matches `apps/public/admin-only.ts` (`/apps/admin/`, `/@dnd-kit/`, `/uplot/`, `CommandPalette.svelte`); verified by forcing an import and by unit tests. Output: `e2e/bundle.spec.ts` finds none of 12 admin-only strings in public JS/CSS/HTML and asserts that every one of them is present in admin, so each string can catch a leak |
| CSP, enforced | 0 console messages, 0 `securitypolicyviolation` events, 0 server reports over every flow |
| CSP report-only (`require-trusted-types-for 'script'; trusted-types kanade-sw`) | 0 reports; no `svelte-trusted-html` policy, `innerHTML`, `insertAdjacentHTML` or data: fonts in any bundle |
| Positive control (`e2e/control.spec.ts`) | Deliberate `style` attribute and `innerHTML` are both reported (`style-src-attr` enforce, `require-trusted-types-for` report) |
| axe (wcag2a/aa, 2.1, 2.2 AA, best-practice) | 0 serious/critical in all 10 faces (5 colourways × light/dark) × public week, list, offline page, admin planner, modal, palette, answers, appearance |
| Lighthouse 13.5 (installed Chrome, headless) | Accessibility 100/100, Best practices 100/100 (both apps). Performance 86 public / 80 admin under simulated slow 4G, uncompressed; CLS 0.21/0.16 from the loading window and font swap |
| Installability | `Page.getInstallabilityErrors` empty apart from Playwright's `in-incognito`; manifests parse with distinct ids `/?app=kanade-public`, `/?app=kanade-admin` and names |
| Build | `bun run build` (both apps + both service workers) 0.77 s wall; each app's Vite build 0.2-0.3 s; `bun install` 2 s cold |
| Tests | vitest 24 passed (6 files); Playwright 43 passed (~78 s, one worker); mock `cargo test` 3 passed |

Lighthouse v12+ has no PWA category; installability comes from the CDP check above.

## Security and CSP findings

Policy as specified, plus `report-uri /csp-report`. `report-to` is omitted in
the mock: when both are present Chrome ignores `report-uri` and batches Reporting
API deliveries for up to a minute, so a zero-report assertion could never see
anything. Production can add `report-to` with `Reporting-Endpoints`.

Workarounds needed; none weakens the policy and no inline style remains:

1. Svelte `compilerOptions.fragments: 'tree'` (5.33+). The default `'html'`
   fills a `<template>` through `innerHTML` behind a pass-through Trusted Types
   policy (`svelte-trusted-html`); `'tree'` builds nodes directly and the policy
   is tree-shaken out.
2. `css: 'external'`; component CSS is extracted by Vite. No Svelte
   transitions or `style:`/`style=` (`svelte/no-inline-styles`, transitions
   disallowed); motion is class-driven `@keyframes` under a reduced-motion
   override. Lint also bans `{@html}`, `innerHTML`, `outerHTML`, `insertAdjacentHTML`.
3. The pre-paint theme bootstrap (inline in v4) is a hashed external classic
   script emitted by `themeBoot()`.
4. v4 inline styles replaced: board track sizing → flex classes; swatch
   colours → per-colourway classes; the nap SVG's `<style>` → component CSS
   (a vitest drift guard compares it with `nap.svg`).
5. `@dnd-kit/dom`'s Feedback, Cursor and PreventSelection plugins inject a
   `<style>` element (nonce-only escape hatch). They are left out, which keeps
   its StyleInjector idle. The planner draws its own ghost (moved via CSSOM
   `style.translate`, which CSP allows) and sets `dragOperation.shape` itself,
   because collision detection needs the shape Feedback would have measured.
6. `navigator.serviceWorker.register` is a Trusted Types script-URL sink:
   registration goes through a `kanade-sw` policy that accepts only `/sw.js`;
   vite-plugin-pwa's inline/auto registration is disabled.
7. `build.assetsInlineLimit: 0`: Vite would otherwise inline small assets as
   data: URIs, which `font-src 'self'` rejects.
8. uPlot needs nothing: canvas plus CSSOM, stylesheet imported as a file.

Headers: hashed `/assets/*` `public, max-age=31536000, immutable`; HTML,
`sw.js`, manifest and icons `no-cache`; API and report sink `no-store`; plus
`nosniff`, `no-referrer`, COOP/CORP `same-origin`, a minimal Permissions-Policy.

## API projections

`GET /api/public/week` returns `PublicWeek`: `starts`, `timezone`, `reset`,
`days`, `generated_at`, and per run `id` (opaque render key; the public
origin has no mutation routes), `day`, `time`, `status`, `bosses`, and an
aggregate `tally` (on/total, as v4's board shows). It omits participant names,
ids and answers, `party` and `version`; e2e asserts the exact key sets.
`GET /api/admin/week` adds `participants` (each with a stable member `id`,
used as the render key because display names repeat; the mock has two
different "Ren"s), `party` and `version`.

The admin store ignores polled snapshots older than the week it holds (a
response that left before the admin's own move committed) and, while a lift
or move is open, buffers the newest snapshot and applies it on release.
"Updated" names the data on screen, not the last response. Unit-tested.

## Offline and updates

- Both workers precache static build output only (`js css html woff2 png
  webmanifest`) and never intercept `/api/`; e2e asserts no `/api/` entry
  exists in Cache Storage after API use. The client also sends `cache: 'no-store'`.
- Navigations are network-first. Offline, public serves the precached
  `offline.html` (nap + "Try again"); admin serves the precached shell, which
  shows its own offline window ("Nothing private is stored on this device").
  Both paths are tested with `context.setOffline(true)` and a reload.
- With service workers blocked (`serviceWorkers: 'block'`) or `?sw=off`
  (which also unregisters), both apps load, poll and work.
- Lazy chunks: if the Answers chunk fails (typically a deploy removed the
  old hashed file), the tab shows "The chart didn't load" with Reload, and
  `vite:preloadError` raises a Reload toast; tested by aborting the chunk.
- Other tabs: when one tab accepts an update, the others see
  `controllerchange` without having asked. They are not reloaded (unsaved
  input would be lost); they get a persistent "updated in another tab ·
  Reload" toast instead.
- Updates are prompt-based: a waiting worker raises a persistent "A new
  version … is ready · Reload" toast; Reload posts `SKIP_WAITING`, and the page
  reloads on `controllerchange` only after that consent. `updateViaCache:
  'none'`; update checks re-run on returning to the tab at most hourly. This
  flow is not automated (it needs two builds); verify manually.

## Keyboard and screen readers

- Planner: every movable card has a Move handle (`aria-describedby` points at
  the visible instructions; `aria-pressed` while lifted). Space/Enter picks
  up, Left/Right changes day, Up/Down changes time by 30 min, Enter/Space
  drops, Escape or leaving the handle cancels. The target column highlights
  and shows "Drop here · 22:30 HCarling + HStar".
- Two live regions. Pointer-drag chatter ("Picked up…", "Over Wed 30.") is
  polite so it never interrupts; results (dropped/cancelled) and keyboard
  steps are assertive because each answers a key press and supersedes the
  last. The assertive region speaks: "Picked up HCarling +
  HStar, Tue 29, 22:00. Left and right arrows change the day…", "HCarling +
  HStar: Mon 28, 22:30.", edge messages ("Wed 30 is the last day of the boss
  week."), "Dropped … on Mon 28, 22:30." or "Move cancelled. … stays on …".
  Repeated sentences are still announced (alternating trailing space).
- The result (server-confirmed) arrives in the polite toast region with an
  Undo button; the toast waits while hovered or focused, and Ctrl/Cmd-Z, the
  page-head "Undo move" button and the palette also undo. Focus returns to the
  moved card's handle after a move, and after an undo from the toast or the
  page-head button (both lose their focused control); tested.
- The run sheet modal offers a third, form-based route (the Move field,
  "wed 21:30") and keeps typed input after a validation or server error.
- Tabs are an ARIA tablist (arrows, Home/End); the palette is a combobox with
  `aria-activedescendant`; dialogs return focus to their opener. VoiceOver was
  not run; the above is from the markup and the Playwright assertions.

## Risks and conditions

1. `@dnd-kit/svelte`/`dom` 0.5.0 is pre-1.0 and the CSP workaround touches
   internals (`dragOperation.shape`, plugin set). Pin exactly, keep the
   pointer-drag e2e test, and keep the keyboard path independent of it.
2. The contrast layer changes v4 colours (chrome in three colourways, `--dim`
   in seven faces, two pill fills, text variants of signals). Approved
   2026-09-24; keep `_contrast.scss` as the single place for such changes.
3. At 1280 px the admin board scrolls sideways slightly because the move
   handle takes width (v4's "scroll rather than squeeze" rule). Revisit the
   handle placement in the real planner.
4. Distinct id/scope/name rely on two origins. Serving both from one origin
   would need path scopes (`/admin/`) and separate SW scopes.
5. CLS 0.16-0.21 from the loading window and font swap; production should
   compress, preload the first-view fonts and reserve the board's space.
6. `fragments: 'tree'` is slower to instantiate than `'html'`; not
   measurable at this size, re-check on a large week.
7. Test tooling: `@axe-core/playwright` is MPL-2.0 (dev-only); bun 1.4
   isolated installs require each package to declare its own workspace deps.
   TypeScript is pinned exactly (`6.0.3`); svelte-check does not accept 7.

Manual checks still needed: iOS Safari add-to-home-screen (icon, standalone
display, `100dvh` frame under the home indicator, `safe-area-inset-bottom`
toasts), iOS offline launch, touch drag on iOS/Android, VoiceOver and
TalkBack passes over the planner, and the two-build update prompt.

## Reproduce

```sh
cd web
bun install --frozen-lockfile
bun run lint && bun run check && bun run test
bun run build            # both apps + service workers, with the public admin-module guard
bun run e2e              # starts its own tools/pwa-mock on :4373/:4374 (never reuses one), clock pinned
bun run measure
bunx playwright test capture                    # placeholder-art captures -> web/e2e/.captures/synthetic/
KANADE_REAL_ART=1 bunx playwright test capture  # local private art -> web/e2e/.captures/real/
cd ../tools/pwa-mock && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

CI runs the same steps in the `web` job of `.github/workflows/ci.yml` on
ubuntu-latest's preinstalled Google Chrome.

## History

- Evaluation (2026-09-24): the stack above, CSP workarounds 1-8, the contrast
  layer, the public/admin projections and the leak guard.
- Parity batch 1: branded masthead, grouped nav with every v4 section as a
  route, login window, now tiles, filters, this/next week, boss portraits and
  entry art (same-origin `/art/*`, null when absent; tests use synthetic
  fixtures in `web/e2e/fixtures/boss`), the full v4 run sheet. No new CSP
  workaround: art is `<img>` from `'self'`, the monogram hue is a CSSOM custom
  property, and the run sheet reports inside itself because a modal makes the
  toast region inert.
- Batch 2: `web/` adopted as production, the mock moved to `tools/pwa-mock`
  with a pinned clock, CI `web` job; Fixed (editor, update-or-keep choice for
  amended runs, retire), Bosses and knowledge, Members and member sheet,
  Reminders, Reset to fixed, the week's hidden past/cancelled runs and the
  run sheet's "this week: −A +B" line. Current sizes: initial JS 30.7 KB
  public / 85.7 KB admin, initial CSS 10.5 / 11.1 KB (gzip).
- Batch 3: route-level code splitting (Week in the initial bundle; other
  pages, the run sheet and the palette on first use) and split stylesheets
  (public ships only public styles); pagers; knowledge schema v2 from the
  tracked `boss/knowledge/` (mock reads YAML with `yaml_serde`); Inbox,
  Extractions (+ rescan jobs), Chat, Limits (v5 backend groups), History
  (git-style timeline, revert / restore-week / revert-member previews,
  checkpoints, run-sheet blame) shaped on `docs/v5/history.md`; login copy for
  Discord OAuth / Tailscale / break-glass token. Sizes: initial JS 30.7 KB
  public / 71.8 KB admin, initial CSS 7.5 / 9.5 KB (gzip); admin total JS
  144 KB across route chunks.
- Batch 4: Config (all non-theme sections), channel access, per-run re-read
  on the board (phones) and in the run sheet, `docs/v5/limits-contract.md`
  and `docs/v5/admin-api.md` (every admin endpoint the PWA calls, each
  marked proposed). The planner's pointer engine (`@dnd-kit/dom` +
  `pointerDrag.ts`) now hydrates on idle or first pointer-over, so
  `@dnd-kit/svelte` is dropped and admin initial JS falls back to ~43 KB;
  keyboard moves never wait for it, and the leak guard's dnd-kit marker is
  `beforedragstart` (minified out of `DragDropManager`). Sizes: initial JS
  30.8 KB public / 43.1 KB admin, initial CSS 7.5 / 9.6 KB (gzip); admin
  total JS 156.4 KB across route + planner chunks.
- Batch 4 review fixes: per-alias admission (`AliasLimit`/`KeyLimits`;
  duplicate/unknown/groupless rows refused or warned naming the row; key sum
  bounded with a shared-key warning), reasoning that resolves inherit
  against published efforts (model-decides takes low/medium/high; stranded
  levels reset to off with a notice), fail-closed cloud/unknown-trust/
  unlisted-alias warnings, self-service redirect copy, files-only reply
  profiles (read-only + publish + ordered roles + reload), chatbot
  unconfigured state, watched-categories env row, Manage-Messages banner,
  retained per-section edits across tab switches, per-channel re-read guard
  (shared by board and sheet) with tolerant polling, planner unmount guard,
  a first press held and replayed once the engine hydrates, narrowed
  `ConfigPatch` (unknown/read-only keys refused with 422), and the
  closed-portal public rule (shell + status + identity only). Also restores
  `@use "runs"` in `public.scss` (the split had dropped it, so public entry
  art rendered raw); an e2e test now holds every `.runcard__art` absolute and
  inside its card on both boards. Planner cards lose the handle column: a
  small always-visible grip, the whole card as drag source (6 px travel, or
  a 250 ms long press on touch), and `M` as the keyboard pick-up. Sizes:
  initial JS 30.8 KB public / 43.6 KB admin, initial CSS 8.5 / 9.7 KB (gzip);
  total CSS 8.8 / 17.5 KB; admin total JS 159.8 KB.
- Batch 5: e2e owns ports 4373/4374 (4383/4384 real art), never reuses a
  server, and checks `/__mock/whoami` for the pinned clock before each test;
  v4 live alignment (short-screen page heads, Config titled in its window,
  Fixed "When" first, 240 px stat floor, the phone week rail back on both
  apps, folded planner help); type sizes on tokens; human week labels in
  History; redirect-feature inbox examples; neutral "Closed · checked" state;
  the frame now clips absolutely positioned descendants (phones could scroll
  the document). Sizes: initial JS 31.6 KB public / 44.4 KB admin, initial
  CSS 8.9 / 10.1 KB (gzip).
