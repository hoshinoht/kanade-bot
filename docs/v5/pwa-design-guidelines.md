# Kanade v5 PWA design migration

Status: migration requirements, not a completed visual review.

## Direction

The PWA changes the delivery architecture, not Kanade's visual identity.
Adopt the established portal CSS design rather than introducing a new theme,
component-library aesthetic, typography system or generic dashboard layout.
Retain all agreed portal workflows except explicitly removed features.

## Authoritative reference

- `legacy/python/bot/api/static/portal.scss` lists the ordered style modules.
- Its `portal/` partials define tokens, base typography, shell, forms, buttons,
  status chips, boss/run presentation, windows, tabs, tables, themes, motion,
  responsive layouts, modals and icons.
- `legacy/python/docs/images/portal-*.png` provide checked-in visual references;
  source styles and a rendered reference resolve gaps or outdated screenshots.
- The existing portal/theme/icon tests record compatibility evidence, not a
  requirement to reproduce their Python implementation or every test verbatim.

Never edit or copy the ignored generated `portal.css` as the source of truth.
The legacy CSS concatenation build remains untouched by the PWA migration;
the v5 build must produce its own versioned static assets without Python.

## Component and layout rules

1. Carry forward existing custom-property tokens and their semantic roles.
   Inventory exact values, typography, spacing, borders, radii and appearance
   option keys before implementing the PWA shell; document deliberate changes.
2. Split styles and components by responsibility. Keep the app shell and page
   orchestration thin; avoid a monolithic stylesheet or page component.
3. Preserve the established operational detail layout: a fixed `100dvh` shell,
   stationary masthead/back navigation/human identity/tab strip, and one tabbed
   window filling remaining height. Only the selected panel scrolls, including
   on narrow screens. Do not replace it with stacked windows or body scrolling.
4. Lead with recognizable names, boss identities and schedule context, not IDs.
   Preserve useful table density and relationships on narrow screens.
5. Reuse existing visual treatments for buttons, forms, tabs, statuses, themes,
   portraits and icons. Critical state must have text/semantics, not color alone.
6. Preserve responsive behavior, keyboard access, visible focus, readable
   contrast and reduced-motion support. Improve accessibility without an
   unrelated redesign; record any necessary visual deviation.

## PWA behavior

- Replace Jinja/HTMX/SSE with the authenticated JSON client and bounded polling;
  keep all mutations on shared Rust application services.
- Preserve input after failures and focus after navigation/dialog actions.
  Include loading, empty, denied, stale/offline, error/retry and destructive
  confirmation/recovery states in the existing visual language.
- Separate public read-only schedule/OAuth and admin capabilities by server
  authorization, not hidden controls. Never cache private API responses or
  credentials in the service worker; cache only explicitly safe app assets.
- Service-worker-disabled operation remains supported. Legacy server-rendered
  no-JavaScript behavior does not imply a no-JavaScript PWA requirement.

## Migration acceptance

Before shell implementation, extend this guide with an extracted token/component
map and representative reference captures. Compare v4 and v5 with matching
synthetic content, themes and narrow/wide viewports. Exercise keyboard/focus,
panel scrolling, failed forms, offline recovery and service-worker updates.
Record intentional differences and rendered evidence; source inspection alone
does not establish visual parity. New PWA states must look native to Kanade.
