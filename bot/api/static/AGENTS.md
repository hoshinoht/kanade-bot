# Portal static assets guide

- There is no Node/Sass build. `portal.js` provides progressive enhancement for dialogs, navigation, filters, SSE, and browser-local appearance settings.
- Edit `portal.scss` and the CSS-compatible underscore partials in `portal/`. Its ordered `@use` list is the concatenation manifest consumed by `bot.portal_styles`.
- Never edit or commit generated `portal.css`; the app can concatenate sources in memory, Docker generates it for the image, and CI validates generation.
- Keep appearance option keys synchronized across `portal.js`, `../templates/partials/theme_boot.html`, and `bot/api/templating.py`.
- Preserve CSS custom-property tokens, responsive phone behavior, reduced-motion handling, and server-rendered functionality without JavaScript.
- Validate with `uv run python -m bot.portal_styles --output /tmp/portal.css` and `uv run pytest -q tests/test_portal_stylesheet.py tests/test_portal_theme.py tests/test_portal_icons.py`.
