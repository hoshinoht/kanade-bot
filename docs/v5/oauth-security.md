# Discord OAuth2 identity: security checklist

Discord OAuth2 establishes identity only: which Discord user is this. Guild
membership, roles and staff status always come from the bot's own gateway data.
This checklist gates admin login (API slice A2) and public exposure. Each item
needs a test. Sources: Discord OAuth2 docs
(https://docs.discord.com/developers/topics/oauth2), RFC 9700 (OAuth 2.0 Security
BCP), RFC 7636 (PKCE), RFC 7009 (revocation), OWASP session/CSRF guidance.
Discord does not officially document PKCE; the unofficial
https://docs.discord.food/topics/oauth2 says S256 is accepted.

Owner decision (2026-09-25): scope `identify` only, one fixed callback URL per
origin, one Discord application for admin login now and a separate application
for the public origin when it opens.

## Admin origin (required now)

1. Authorization-code flow only, confidential client; secret read from a file,
   sent with HTTP Basic, redacted in `Debug`. Never the implicit grant.
2. `state`: at least 128 CSPRNG bits, stored server-side as a hash, bound to a
   short-lived pre-auth cookie (about 10 min), deleted on first use whether the
   login succeeds or fails. It is the primary login-CSRF defence.
3. PKCE S256 as defence in depth (verifier stored with `state`). A live check
   confirms Discord rejects a wrong verifier; if not, record that PKCE is not
   enforced and keep relying on `state`.
4. One fixed `redirect_uri` per origin from config, never derived from the
   request, sent identically to the authorize and token endpoints.
5. Scope exactly `identify`; reject a broader returned scope.
6. Identity only from `GET /users/@me` with the fresh token; ignore any
   client-supplied user id; reject `bot: true`.
7. Store no tokens: after `/users/@me`, revoke the token (best effort; a failure
   is logged without the token) and discard it. No refresh tokens.
8. A code works once: a replayed callback fails locally because `state` is gone.
9. Callback errors never echo `error`, `error_description` or `state`.
10. Post-login `next`: relative same-origin paths only (starts with `/`, not
    `//` or `/\`), stored server-side with `state`.
11. New session id at login, pre-auth record deleted, idle and absolute
    timeouts, server-side logout.
12. `__Host-` cookies, with separate names for admin, public and pre-auth.
    The pre-auth cookie is `SameSite=Lax` because the callback is a cross-site
    top-level navigation. A `Strict` session cookie needs the callback to answer
    200 and then navigate within the origin.
13. The staff gate reads bot data: roles plus computed Administrator permission
    and guild ownership (interactions get `is_guild_admin` directly; the web
    must compute it). Short-TTL recheck, and sessions are invalidated on
    member-remove or role-update gateway events. *Status (A3):* the staff
    gate reads persisted member rows (`StoreGuildMembers`: stored roles and
    `is_guild_admin`, owner from `GuildAvailable`), and
    `api::auth::roster::on_roster_update` persists `BotEvent::Roster` and
    calls `member_left` / `member_changed`. Still open: `RosterUpdate::Seen`
    carries no role list or computed Administrator permission, so stored
    roles only change when that seam lands (see `runtime-bootstrap.md`
    composition gaps); a departure clears them at once.
14. Rate limits on `/login` and the callback, per client IP and globally;
    Discord 429s honour `retry_after` and fail closed. *Recorded exception:*
    for the break-glass token (login and bearer) the global bucket counts and
    refuses wrong tokens only, so guesses from many addresses cannot lock out
    the right token; its ≥ 32-byte entropy makes online guessing moot. Per-IP
    limits apply to every attempt.
15. Never log the code, state, verifier, tokens or cookies; log the user id,
    outcome and request id.

## Before public exposure

16. Full origin separation: its own redirect URI (preferably its own Discord
    application) and separate sessions (or a checked realm); cookies and
    `state` never cross origins.
17. Eligibility: in the guild and holding the bossing role; otherwise a
    neutral denial with no session; revalidated within minutes and on events.
18. Behind Cloudflare Tunnel, trust `CF-Connecting-IP` only from the
    cloudflared peer, so per-IP limits work.
19. `prompt` gives no re-authentication guarantee; `none` is fine for identity.
20. Privacy: keep only the user id (plus display name and avatar hash if
    shown), no `email`; retention for session and audit rows; a short notice on
    the login page.
21. Public logout and any public POST check Origin/Sec-Fetch-Site and a CSRF
    token.
22. Session hardening (owner decision 2026-09-25) for public sessions:
    - a member-facing "Signed-in devices" list with sign out one or all, and an
      audit event for each new sign-in;
    - shorter lifetimes than admin (for example 30 min idle, 8 h absolute;
      final values set with the public plan);
    - a fresh Discord sign-in for member writes once the last sign-in is older
      than a short window (see item 24);
    - a client-IP change does not end the session (mobile networks change
      addresses constantly) but rotates the session id and re-checks
      eligibility before the next request is served.

## Later

23. Link Discord ids to roster members by snowflake only, never by name.
24. Public member writes need per-member authorization on every write, CSRF and
    Idempotency-Key, stricter cookies, short absolute timeouts and a fresh OAuth
    round trip for sensitive changes (Discord has no `max_age`).
25. Alert on callback failure spikes.
26. `Cache-Control: no-store` and `Referrer-Policy: no-referrer` on login and
    callback responses.
