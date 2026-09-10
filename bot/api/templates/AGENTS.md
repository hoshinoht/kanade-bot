# Portal template guide

- Portal pages extend `base.html`; `login.html` is a standalone document that includes `partials/theme_boot.html`. Shared components, icons, flash messages, navigation, and row/run fragments live in `partials/`.
- Keep the portal server-rendered and usable when HTMX/CDN JavaScript is unavailable. Search, paging, and mutations remain real links/forms with HTMX as progressive enhancement.
- Mutating handlers return a refreshed fragment for HTMX and a safe redirect/flash for ordinary forms. Match each `hx-target`/swap boundary to the server-rendered partial root.
- A run article is the replacement unit after run actions; avoid rendering the same run through multiple competing template paths.
- Add reusable display logic to `macros.html` or `templating.py` rather than duplicating it across pages. Keep inline SVG definitions centralized in `partials/icons.html`.
- Validate with the relevant `tests/test_api_portal.py` or `tests/test_portal_*.py` file.
