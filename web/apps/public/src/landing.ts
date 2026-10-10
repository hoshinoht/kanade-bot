/**
 * Sign-in plumbing for the member portal (member-auth-contract §1, §6): where
 * the Discord flow starts, and what the callback's `?login_error=` means here.
 */

/** Same rule as the server's `next`: a path on this origin, never `//`, `/\`, a scheme or the API. */
export function safeNext(next: string | null | undefined): string {
  if (!next) return '/';
  const ok =
    next.startsWith('/') &&
    !next.startsWith('//') &&
    !next.startsWith('/api/') &&
    next.length <= 512 &&
    // Printable ASCII only, as the server checks.
    /^[\x21-\x7e]+$/.test(next) &&
    !next.includes('\\');
  return ok ? next : '/';
}

/** Where the Discord flow starts: a full-page navigation, not a fetch. */
export function discordStart(next: string): string {
  return `/api/public/auth/discord/start?next=${encodeURIComponent(safeNext(next))}`;
}

/**
 * What the app shows for the callback's code: `not_eligible` is the neutral
 * Denied page, `closed` the Closed one; the rest are notices on Sign in that
 * say what to do (contract §1's closed set): `state` took too long,
 * `rate_limited` too many tries, `unavailable` member data or Discord is
 * down (nobody was refused); `denied`, `discord` and anything else simply
 * didn't finish.
 */
export type Landing = 'none' | 'denied' | 'closed' | 'failed' | 'expired' | 'limited' | 'unavailable';

const NOTICES: Record<string, Landing> = { not_eligible: 'denied', closed: 'closed', state: 'expired', rate_limited: 'limited', unavailable: 'unavailable' };

export function landingOf(search: string): Landing {
  const code = new URLSearchParams(search).get('login_error');
  if (code === null) return 'none';
  // Own keys only: a code such as `constructor` is no landing.
  return Object.hasOwn(NOTICES, code) ? NOTICES[code]! : 'failed';
}

/** The same address without `login_error`, so a reload does not repeat the outcome. */
export function withoutLoginError(path: string, search: string, hash: string): string {
  const query = new URLSearchParams(search);
  query.delete('login_error');
  const rest = query.toString();
  return `${path}${rest ? `?${rest}` : ''}${hash}`;
}
