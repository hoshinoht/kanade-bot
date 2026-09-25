# Material 3 Expressive verification for the Kanade admin PWA

Checked **2026-09-26** against the live Material 3 site in Zen Browser, official Android sources, MDN, and browser support data. This is a design reference, not an instruction to use Material's visual theme or component libraries. M3 values are in **dp/sp**; copying them as CSS pixels is an implementation choice, not a web token guarantee. Material 3 pages generally do **not** expose a page-level “last updated” date. Each source line below says so rather than treating a crawl date or an in-page announcement as a revision date. Short quotations are exact; the rest is paraphrase.

## A. Values we could not verify before

### 1. Emphasized typography

The 15 emphasized roles keep the baseline sizes. Official Android styles give these exact values; tracking below is the Android `letterSpacing` unit (an **em fraction**, not px). “Medium” and “Bold” are the source's weight names; the numeric weight depends on the selected typeface. This is an Android implementation of M3, not a published web CSS token set.

| Role | Size | Line height | Weight | Tracking |
|---|---:|---:|---|---:|
| Display large | 57sp | 64sp | Medium | 0 |
| Display medium | 45sp | 52sp | Medium | 0 |
| Display small | 36sp | 44sp | Medium | 0 |
| Headline large | 32sp | 40sp | Medium | 0 |
| Headline medium | 28sp | 36sp | Medium | 0 |
| Headline small | 24sp | 32sp | Medium | 0 |
| Title large | 22sp | 28sp | Medium | 0 |
| Title medium | 16sp | 24sp | Bold | 0.009375 |
| Title small | 14sp | 20sp | Bold | 0.00714286 |
| Body large | 16sp | 24sp | Medium | 0.009375 |
| Body medium | 14sp | 20sp | Medium | 0.01785714 |
| Body small | 12sp | 16sp | Medium | 0.03333333 |
| Label large | 14sp | 20sp | Bold | 0.00714286 |
| Label medium | 12sp | 16sp | Bold | 0.04166667 |
| Label small | 11sp | 16sp | Bold | 0.04545455 |

Use emphasized roles for selection, actions, headlines, or editorial hierarchy; the baseline and emphasized scales can coexist. M3 examples include selected list/menu text, badges, prominent actions, and extended FABs. **Unconfirmed:** M3 does not state a maximum number of emphasized elements per screen on the live typography page. The blog's limit of 1–2 hero moments is a **product-level** limit, not a typography quota.

Sources: [M3 type scale](https://m3.material.io/styles/typography/type-scale-tokens) (last updated: **not stated**); [official Android emphasized style values](https://github.com/material-components/material-components-android/blob/master/lib/java/com/google/android/material/typography/res/values/styles.xml) and [typography guide](https://github.com/material-components/material-components-android/blob/master/docs/theming/Typography.md) (file-level last update: **not confirmed**).

### 2. Layout margins, gaps, and pane widths

| Width class | Window width | Side margins | Pane gap/spacer | Recommended visible panes | Fixed pane guidance |
|---|---:|---:|---:|---:|---|
| Compact | <600dp | 16dp | not applicable | 1 | none |
| Medium | 600–839dp | 24dp | 24dp if split | 1; 2 only for low-density content | equal 50/50 if split |
| Expanded | 840–1199dp | 24dp | 24dp | 2; 1 for dense content | 360dp default |
| Large | 1200–1599dp | 24dp | 24dp | 2; 1 for dense content | 412dp default |
| Extra-large | ≥1600dp | 24dp | 24dp | 2; optional 1 or 3 | 412dp; optional side sheet max 400dp |

At least one pane must be flexible. M3 offers 360dp and 412dp as fixed-pane snap widths and remembers user resizing. **Unconfirmed:** there is no universal minimum or maximum width for every pane on these pages; 400dp is the maximum for the optional extra-large side sheet, not every pane. The 24dp inter-pane spacing is directly documented in the class pages.

Sources: [breakpoints](https://m3.material.io/foundations/layout/breakpoints/overview) (last updated: **not stated**); [compact](https://m3.material.io/foundations/layout/breakpoints/compact), [medium](https://m3.material.io/foundations/layout/breakpoints/medium), [expanded](https://m3.material.io/foundations/layout/breakpoints/expanded), [large and extra-large](https://m3.material.io/foundations/layout/breakpoints/large-extra-large), [panes](https://m3.material.io/foundations/layout/scaffold/panes) (last updated for each: **not stated**). The [layout overview](https://m3.material.io/foundations/layout/layout-overview/overview) contains a **May 2026 update announcement**, not a page-level last-updated field.

### 3. List row → detail motion

M3 shows container transform for a list/card expanding into a full-screen view, but its current transition guidance says: “Don't use container transform in apps with deep hierarchies, the motion becomes excessive.” It explicitly calls this a poor fit for utility-focused navigation. For frequent admin list → detail navigation, use a simple **forward/backward** transition. On two-pane layouts, keep the list stable and update the detail pane; a subtle content change is enough. Reserve container transform for a rare hero view with a persistent shared container.

Transitions still use the legacy easing/duration system, even though Expressive component interactions use springs. Suggested pairs: emphasized 500ms on-screen, 400ms enter, 200ms exit; standard 300ms on-screen, 250ms enter, 200ms exit. For web, M3 says the Standard easing set is a fallback where Emphasized is unavailable. CSS Standard curves: `cubic-bezier(0.2, 0, 0, 1)`; decelerate `cubic-bezier(0, 0, 0, 1)`; accelerate `cubic-bezier(0.3, 0, 1, 1)`. These are **general transition defaults**, not an exact prescribed list-row timing. M3 does not recommend the Web View Transitions API specifically on these pages; using it would be our implementation choice. “Shared axis” is not the current named M3 transition pattern here; the site calls the common hierarchical pattern “forward and backward.”

Sources: [transition patterns](https://m3.material.io/styles/motion/transitions/transition-patterns), [applying transitions](https://m3.material.io/styles/motion/transitions/applying-transitions), [easing/duration guidance](https://m3.material.io/styles/motion/easing-and-duration/applying-easing-and-duration), [easing tokens](https://m3.material.io/styles/motion/easing-and-duration/tokens-specs) (last updated for each: **not stated**).

### 4. “Expressive design tactics” article

The [article](https://m3.material.io/blog/building-with-m3-expressive) lists **seven** tactics. Its publication date is **May 13, 2025**; a last-updated date is **not stated**. Only tactics 1, 2, and 4 have explicit Do/Caution callouts; inventing one for the others would misrepresent the article.

| Tactic | Article's direction | Explicit Do / caution, where supplied |
|---|---|---|
| 1. Use a variety of shapes | Mix shapes/corner radii to establish focus and grouping. | Do: “Break from the surrounding shape style to draw attention to a particular element.” Caution: smaller shapes can make essential actions look less important. |
| 2. Apply rich and nuanced colors | Use contrast among surface and action roles to set priority. | Do: “Use contrast to emphasize the main takeaway or element.” Caution: without contrast elements blend together. |
| 3. Guide attention with typography | Emphasize important headlines/actions and establish hierarchy. | No explicit Do/Don't callout. |
| 4. Contain content for emphasis | Group related content; give important tasks space and prominence. | Do: group similar content informatively. Caution: ungrouped information can blend together. |
| 5. Add fluid and natural motion | Use shape morphs, surface effects, springs, or custom micro animations. | No explicit Do/Don't callout. |
| 6. Leverage component flexibility | Configure app bars, toolbars, buttons and other components for context. | No explicit Do/Don't callout. |
| 7. Combine tactics to create hero moments | Choose emotionally meaningful moments. | Limit: 1–2 such moments per product; more can distract. No paired Do/Don't. |

**Unconfirmed:** the article does not give a separate productivity/utility-app tactic. For Kanade, the directly relevant separate rule is M3's Standard motion guidance for quick utility-focused transitions; the article's hero moments are optional.

## B. Components to hand-build

### 5. Docked toolbars

All current toolbars have a default **64dp** height, centered controls, and at least **16dp** outside padding. Docked toolbar control spacing defaults to **32dp**; interactive targets are at least **48×48dp**. Actions that do not fit move into a trailing overflow menu; M3 sets **no fixed maximum action count**. Standard color keeps attention on content; vibrant gives controls higher emphasis, useful for a temporary mode such as editing. The baseline docked toolbar is placed at the **bottom of the window**. A top-of-pane action row is therefore a **custom adaptation**; M3's adaptive examples allow flexible placement on large/web canvases, but do not prescribe a top-docked pane toolbar. A 48px titlebar containing actions does not satisfy the default 64dp docked-toolbar geometry simply by naming it one.

Sources: [toolbar specs](https://m3.material.io/components/toolbars/specs), [toolbar guidelines](https://m3.material.io/components/toolbars/guidelines) (last updated for each: **not stated**).

### 6. Button groups and split buttons

Standard groups join related **actions** and have visual gaps; connected groups join **selectable related options** such as sorting or view modes. A connected group may be single- or multi-select and may require a selection. A non-toggle set should not be presented as a connected selection group. In Expressive, connected groups replace segmented buttons: the [segmented-button page](https://m3.material.io/components/segmented-buttons/overview) says they are “no longer recommended.”

| Size | Container height | Standard inner gap | Connected inner gap | Connected inner corner | Square button corner | Pressed button corner |
|---|---:|---:|---:|---:|---:|---:|
| XS | 32dp | 18dp | 2dp | 4dp | 12dp | 8dp |
| S | 40dp | 12dp | 2dp | 8dp | 12dp | 8dp |
| M | 56dp | 8dp | 2dp | 8dp | 16dp | 12dp |
| L | 96dp | 8dp | 2dp | 16dp | 28dp | 16dp |
| XL | 136dp | 8dp | 2dp | 20dp | 28dp | 16dp |

The normal round button corner is **Full** at every size. Selected/toggled buttons change color and shape; standard groups also change the selected button's width and adjacent widths, while connected groups affect their own shape. XS/S visual buttons still need **48×48dp targets**. Heights are corroborated by [official Compose generated button tokens](https://android.googlesource.com/platform/frameworks/support/+/376128c13e708e85a77d70bc7e0ce0cb39a6bc8c%5E2..376128c13e708e85a77d70bc7e0ce0cb39a6bc8c/) and [XL split-button tokens](https://android.googlesource.com/platform/frameworks/support/+/01b5590dcc8e67b98207c02bfa52275b3ae5bc20%5E2..01b5590dcc8e67b98207c02bfa52275b3ae5bc20/) (commit dates/page last updated: **not confirmed**).

Split buttons pair a primary action with a trailing menu. Their gap is **2dp**; inner corners are XS/S/M **4dp**, L **8dp**, XL **12dp**. The trailing button rotates **180°** when expanded and gets a **10% state layer**; its color does not switch to a selected color. Keep the main label to one or two words and the menu **4dp** from the split button.

Sources: [button-group specs](https://m3.material.io/components/button-groups/specs), [button-group guidelines](https://m3.material.io/components/button-groups/guidelines), [button specs](https://m3.material.io/components/buttons/specs), [split-button specs](https://m3.material.io/components/split-button/specs), [split-button guidelines](https://m3.material.io/components/split-button/guidelines), [segmented buttons](https://m3.material.io/components/segmented-buttons/overview) (last updated for each: **not stated**).

### 7. Loading and progress

Show content immediately if the wait is **under 200ms**; use a loading indicator for **200ms–5s**; for waits expected to exceed **5s**, start with a progress indicator and do not swap indicators mid-wait. Loading indicator default diameter is **48dp**, with supported range **24–240dp**. Use uncontained on a plain surface; a containing surface is useful over other content or for pull-to-refresh. Center it within the area waiting, place incremental-list loading in the future content area, and put a compact indicator in a busy button while preserving its meaning/target. Its active animation morphs through seven shapes. Use a descriptive accessible name and `role="progressbar"`; ensure **3:1** indicator contrast, and offer a button alternative to pull-to-refresh. For reduced motion, replace continuous morphing with a static or gently changing status and retain the accessible status text; that final choice is an **inference** from M3 motion accessibility guidance, not a specific loading-page token.

Sources: [loading guidelines](https://m3.material.io/components/loading-indicator/guidelines), [loading accessibility](https://m3.material.io/components/loading-indicator/accessibility), [progress guidelines](https://m3.material.io/components/progress-indicators/guidelines), [transition accessibility](https://m3.material.io/styles/motion/transitions/applying-transitions) (last updated for each: **not stated**).

### 8. Shape

| Corner token | Radius |
|---|---:|
| None | 0dp |
| Extra small | 4dp |
| Small | 8dp |
| Medium | 12dp |
| Large | 16dp |
| Large increased | 20dp |
| Extra large | 28dp |
| Extra large increased | 32dp |
| Extra extra large | 48dp |
| Full | 50% / fully round |

M3 uses state changes such as round-to-square on press or selection; focused elements need a visible focus indicator, not shape alone. Nest corners with the optical rule `inner radius = outer radius − padding`. Its separate library has **35 iconic shapes** for decorative crops, avatars, and morphs; loading and buttons use morphs to communicate motion/state. The [official Compose `MaterialShapes` source](https://android.googlesource.com/platform/frameworks/support/+/c8a071114c193cd7b43a05ba1489e72d21f3b833/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/MaterialShapes.kt) exposes all 35 names: Circle, Square, Slanted, Arch, Fan, Arrow, SemiCircle, Oval, Pill, Triangle, Diamond, ClamShell, Pentagon, Gem, VerySunny, Sunny, Cookie4Sided, Cookie6Sided, Cookie7Sided, Cookie9Sided, Cookie12Sided, Ghostish, Clover4Leaf, Clover8Leaf, Burst, SoftBurst, Boom, SoftBoom, Flower, Puffy, PuffyDiamond, PixelCircle, PixelTriangle, Bun, Heart. **Unconfirmed:** M3 does not prescribe a web morph implementation. Avoid decorative morphing in dense text rows.

Sources: [corner scale](https://m3.material.io/styles/shape/corner-radius-scale), [shape morph](https://m3.material.io/styles/shape/shape-morph), [shape overview](https://m3.material.io/styles/shape/overview), [Expressive article](https://m3.material.io/blog/building-with-m3-expressive), [official shape definitions](https://android.googlesource.com/platform/frameworks/support/+/c8a071114c193cd7b43a05ba1489e72d21f3b833/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/MaterialShapes.kt) (M3 pages last updated: **not stated**; article published **May 13, 2025**, last updated **not stated**; source-file date **not confirmed**).

### 9. Dense lists and tables

M3's density page explicitly permits an **opt-in compact density**: default target at least **48×48 CSS px**, with density steps **0, −1, −2, −3** typically removing **4dp** of height/padding per step. A smaller visual/target arrangement is allowed cautiously where dense scanning benefits, but keep the density control itself at 48×48 and do not auto-change density from viewport width. It recommends density for lists, tables, and forms; avoid it for focused dialogs, menus, and snackbars. List shape specs: unselected contained list item corners **4dp inner / 16dp outer**; selected corners **16dp all around**, with selected row container using primary-container/on-primary-container roles. Use full-row containment for a selection mode. M3 also says a **single-action navigation row should not persist as selected**. A two-pane list-detail pattern does show the current detail's corresponding active row; treat that as active navigation context, separate from multi-selection. For Kanade, use clear active-row containment without implying bulk selection.

Sources: [density](https://m3.material.io/foundations/layout/grids-spacing/density), [list specs](https://m3.material.io/components/lists/specs), [list guidelines](https://m3.material.io/components/lists/guidelines), [list-detail](https://m3.material.io/foundations/layout/canonical-examples/list-detail) (last updated for each: **not stated**).

## C. Layout decisions

### 10. Canonical layouts

List-detail: **1 pane** compact; **1 recommended / 2 optional** medium; **2** expanded and larger. Medium two-pane suits light browsing with quick switching; dense information or deep focus stays one pane. At compact width, show list **or** detail; detail occupies the pane and provides a **Back** affordance. Expanded list-detail has fixed list and flexible detail. A supporting pane holds secondary material meaningful only alongside a primary pane; below the focus pane on compact/medium and beside it at larger widths (fixed-pane guidance **360dp**). Feed layouts stack at compact and gain columns as space allows. In the fixed `100dvh` Kanade shell, these rules change pane content inside the existing non-scrolling chrome; they do not require page-body scroll.

Sources: [canonical overview](https://m3.material.io/foundations/layout/canonical-examples/overview), [list-detail](https://m3.material.io/foundations/layout/canonical-examples/list-detail), [supporting pane](https://m3.material.io/foundations/layout/canonical-examples/supporting-pane), [feed](https://m3.material.io/foundations/layout/canonical-examples/feed), [medium](https://m3.material.io/foundations/layout/breakpoints/medium) (last updated for each: **not stated**).

### 11. Navigation rail, tabs, and bottom navigation

The current Expressive collapsed rail is **96dp** wide in the official Android implementation (raised from baseline **80dp**), for **3–7 destinations**; expanded rail exposes labels and submenu content, replacing the drawer. **Unconfirmed:** a universal expanded-rail width in the live M3 accessible specs; its width is shown in a diagram, and Android allows configuration. Rail is for top-level destinations on medium/large canvases. Tabs switch **peer views within one destination**; a top app bar supplies title/search/actions and is not itself a destination navigator. These can coexist in a web app if each has a distinct job. M3 does **not** publish a blanket prohibition on bottom navigation in data-heavy apps; the decision to avoid it in Kanade is a product judgment to preserve vertical space and keep the fixed shell calm.

Sources: [rail overview](https://m3.material.io/components/navigation-rail/overview), [rail specs](https://m3.material.io/components/navigation-rail/specs), [rail guidelines](https://m3.material.io/components/navigation-rail/guidelines), [official Android rail guide](https://github.com/material-components/material-components-android/blob/master/docs/components/NavigationRail.md), [tabs overview](https://m3.material.io/components/tabs/overview) (last updated for each: **not stated**; Android file last update **not confirmed**).

### 12. Top app bar

M3 Expressive offers small, search, medium-flexible, and large-flexible app bars; old medium/large variants are no longer recommended. Flexible variants can collapse to small on scroll. Small suits dense layouts and scrolled state. The official Compose small-app-bar token is **64dp**. M3 asks for default height rather than an artificially shortened app bar. Search width uses all available width until **312dp**, then **50%**; at most **2 trailing icons on mobile** or **4 on large screens**. **Unconfirmed:** exact 2026 *search* app-bar height—the live specs present it as an image without accessible dimensions. Kanade's permanently fixed masthead/titlebar should be treated as custom shell chrome; a collapse-on-document-scroll recipe does not directly apply when only the selected panel scrolls. If used, the collapse trigger must be the panel's scroll position.

Sources: [app-bar specs](https://m3.material.io/components/app-bars/specs), [app-bar guidelines](https://m3.material.io/components/app-bars/guidelines), [official Compose small-app-bar token](https://android.googlesource.com/platform/frameworks/support/+/160825094a81825468a95b115bfb1b541e549856%5E%21/), [official Android search guide](https://github.com/material-components/material-components-android/blob/master/docs/components/Search.md) (last updated for M3 pages: **not stated**; source-file dates **not confirmed**).

## D. Accessibility and web platform

### 13. Reduced motion

M3 says that with the platform reduced-animation setting, transitions should “Use subtle fades instead of intense sliding or scaling animations” and “Disable decorative effects like parallax or shape morphing.” Its general transition guidance also says common transitions should avoid bouncy springs. For this PWA, use `prefers-reduced-motion: reduce` to suppress row-to-detail travel, button shape morph, and decorative looping; keep instant/short state feedback plus accessible loading text. The last sentence is a web implementation inference. **Unconfirmed:** a specific M3 duration for reduced-motion springs or loading loops; the pages do not provide one.

Sources: [applying transitions](https://m3.material.io/styles/motion/transitions/applying-transitions), [loading accessibility](https://m3.material.io/components/loading-indicator/accessibility) (last updated for each: **not stated**); [MDN reduced-motion media query](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/At-rules/@media/prefers-reduced-motion) (**last modified Jun 10, 2026**).

### 14. Official web implementations

As checked in September 2026, the [Material Web repository](https://github.com/material-components/material-web) still says “MWC is in maintenance mode pending new maintainers.” The team's [June 10, 2024 announcement](https://github.com/material-components/material-web/discussions/5642) says no new features/components are planned and staffing was reassigned; the team's [May 15, 2025 answer](https://github.com/material-components/material-web/discussions/5806) explains that M3 pages mark web unavailable for that reason. `@material/web` (Lit web components) is therefore not an official Expressive component set. Angular Material is an official Angular component library with M3 theming, but **no verified official implementation of the full new Expressive set** (such as M3's docked toolbar, connected group, and loading indicator) was found there. That is a scoped absence finding, not a claim that Angular Material has no M3 features. Repository page last updated: **not stated**; announcement dated **June 10, 2024**; answer dated **May 15, 2025**. [Angular Material component index](https://material.angular.dev/components/categories) (last updated: **not stated**).

### 15. iOS keyboard, `interactive-widget`, and `dvh`

`interactive-widget=resizes-content` requests that the keyboard resize the **layout viewport**; MDN says the default `resizes-visual` only resizes the **visual viewport**, which does not reflow page layout. [Can I use](https://caniuse.com/mdn-html_elements_meta_name_viewport_interactive-widget_resizes-content) currently marks **Safari on iOS unsupported** (data checked **2026-09-26**; page does not state last update). This is a correction to treating the meta tag as an iOS solution. `100dvh` follows the dynamic **layout viewport** as browser UI changes; it is **not guaranteed to shrink for the keyboard** when only the visual viewport changes. MDN's [VisualViewport reference](https://developer.mozilla.org/en-US/docs/Web/API/VisualViewport) explicitly says the keyboard may shrink the visual viewport without affecting layout and offers `window.visualViewport` resize/scroll events. Keep the `100dvh` shell and handle keyboard overlap with measured visual-viewport height/insets and real-device Safari tests, especially for bottom controls. The exact Safari/PWA standalone behavior across iOS versions is **not confirmed** here.

Sources: [MDN viewport meta](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Elements/meta/name/viewport) (**last modified Sep 23, 2026**), [MDN VisualViewport](https://developer.mozilla.org/en-US/docs/Web/API/VisualViewport) (**last modified Aug 12, 2026**), [MDN viewport units](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Values/length) (**last modified Jul 8, 2026**), [Can I use compatibility](https://caniuse.com/mdn-html_elements_meta_name_viewport_interactive-widget_resizes-content) (last updated: **not stated**, checked **2026-09-26**).

## Corrections to `docs/v5/m3-expressive-evaluation.md`

1. **Title bar as docked toolbar:** M3's docked default is bottom placement with **64dp** height and **48×48dp** targets. Folding controls into an existing ~48px titlebar is a valid Kanade-specific design, but should not be called literal M3 docked-toolbar compliance.
2. **Connected buttons or segmented controls:** Expressive guidance says segmented buttons are **no longer recommended**; use a connected button group for selectable related options.
3. **Density:** the evaluation says targets stay ≥48px even for compact density. M3's density page allows cautious **opt-in reductions below 48px** in dense scanning contexts; 48px remains the safe default and the density control stays 48px.
4. **Container transform as list-detail option:** current transition guidance specifically cautions against repeated container transforms in deep, utility-focused navigation. Favor a simple forward/backward transition for routine admin rows.
5. **`interactive-widget=resizes-content` on phone:** current compatibility data marks **iOS Safari unsupported**. It cannot be the only keyboard-overlap fix for the fixed shell.

The evaluation's compact/medium/expanded breakpoints, one-pane medium rule for dense content, 24dp pane gap, and 200ms/5s loading split are supported by the current pages.
