# Login (admin sign-in)

**Purpose:** sign in (Discord, token or Tailscale) outside the rail shell.

**Board:** `HeroLogin` (pair: hero-login). `HeroPublic` stays skipped (public
portal, post-beta). `[DR 2026-10-04]`

## As built

Code: `apps/admin/src/pages/LoginPage.svelte`; `_gate.scss`.

- No navigation; hero (`gate__hero`) with the bot identity (avatar or
  monogram fallback, `--fs-avatar`).
- "Tonight" / "Today" strip (from 17:00 "Tonight") from
  `GET /api/admin/auth/tonight`: time and bosses only, no names, answers or
  party (public-safe field). `[DR 2026-10-04]`, commit e66905d
- Sign-in buttons use `PendingLabel` while waiting.

## Public app

The public portal is a separate product on its own origin, with its own
masthead and 1180 px cap; see `../../public-portal-plan.md` (linked, not
merged).
