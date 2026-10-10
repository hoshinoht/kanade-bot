/**
 * Sign-in plumbing (admin-api "Sign-in and sessions"): where to return after
 * signing in, and the server's `?login_error=` codes in words.
 */

/** Same rule as the server's `safe_next`: a path on this origin, never `//`, a scheme or the API. */
export function safeNext(next: string | null | undefined): string {
  if (!next) return '/';
  const ok =
    next.startsWith('/') &&
    !next.startsWith('//') &&
    !next.startsWith('/api/') &&
    !next.startsWith('/login') &&
    next.length <= 512 &&
    // Printable ASCII only, as the server checks.
    /^[\x21-\x7e]+$/.test(next) &&
    !next.includes('\\');
  return ok ? next : '/';
}

/** The sign-in page, returning to `path` afterwards. */
export function loginHref(path: string): string {
  const next = safeNext(path);
  return next === '/' ? '/login' : `/login?next=${encodeURIComponent(next)}`;
}

/** Where the Discord flow starts: a full-page navigation, not a fetch. */
export function discordStart(next: string): string {
  return `/api/admin/auth/discord/start?next=${encodeURIComponent(safeNext(next))}`;
}

/** The callback's `/?login_error=<code>` outcomes. */
export const LOGIN_ERRORS: Record<string, string> = {
  state: 'That sign-in link expired or was already used. Sign in with Discord again.',
  denied: 'Discord sign-in was cancelled. Sign in again when you are ready.',
  forbidden: "Your Discord account isn't recognised as a Kanade admin. Ask a guild admin, or use the admin token.",
  discord: "Discord didn't complete the sign-in. Try again in a moment.",
  unavailable: 'Discord sign-in is unavailable right now. Try again later, or use the admin token.',
  rate_limited: 'Too many sign-in attempts. Wait a minute, then try again.',
};

export function loginErrorText(code: string): string {
  return code ? (LOGIN_ERRORS[code] ?? 'Sign-in did not complete. Try again.') : '';
}
