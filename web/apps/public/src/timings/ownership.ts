// The member's weekly timings and their ownership (boards MyRuns-Timings-Owner,
// PhoneMyRuns-Timings-Owner): order, names and words. Pure functions over
// `MemberTimings`; "now" is the server's clock (`generated_at`), never the
// browser's.
import type { Member, MemberOwnerRequest, MemberTiming, PublicSession, PublicSessionRow } from '@kanade/api-types';
import { ApiRequestError } from '@kanade/client';

/** `MemberTiming.weekday` is 0 = Monday. */
export const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const;

/** "Fri 22:00": a timing as the boards and Discord name it. */
export const whenWords = (timing: Pick<MemberTiming, 'weekday' | 'time'>): string => `${WEEKDAYS[timing.weekday] ?? '?'} ${timing.time}`;

/** "XKalos", "HCarling + HStar": the bosses as Discord's ownership notices name them. */
export const tokenWords = (timing: Pick<MemberTiming, 'bosses'>): string => timing.bosses.map((b) => b.token).join(' + ');

/**
 * In boss-week order from `startDay` (the week's first weekday, e.g. "Thu"),
 * then by time; an unknown start day counts from Monday.
 */
export function sortTimings(timings: MemberTiming[], startDay: string | undefined): MemberTiming[] {
  const start = Math.max(0, WEEKDAYS.indexOf((startDay ?? 'Mon') as (typeof WEEKDAYS)[number]));
  const offset = (t: MemberTiming) => (t.weekday - start + 7) % 7;
  return [...timings].sort((a, b) => offset(a) - offset(b) || a.time.localeCompare(b.time) || a.id.localeCompare(b.id));
}

/** Other members' open asks on the member's own timing. */
export const incoming = (timing: MemberTiming): MemberOwnerRequest[] => (timing.you_own ? timing.requests.filter((r) => r.status === 'open' && !r.mine) : []);

/** The member's own open ask on someone else's timing. */
export const pendingAsk = (timing: MemberTiming): MemberOwnerRequest | null => timing.requests.find((r) => r.status === 'open' && r.mine) ?? null;

/** "19 h", "40 min": what is left of an ask by the server's clock, whole hours from an hour up. */
export function expiresWords(request: Pick<MemberOwnerRequest, 'expires_at'>, now: string): string {
  const left = Math.max(0, (Date.parse(request.expires_at) - Date.parse(now)) / 60_000);
  if (!Number.isFinite(left)) return '';
  // An ask lives 24 h: hours read better than "1 day" (boards: "23 h").
  return left >= 60 ? `${Math.floor(left / 60)} h` : `${Math.round(left)} min`;
}

/**
 * The fresh-sign-in window owner changes need (server `fresh_write`, 5–30
 * min): this device's sign-in time and the window's length, when the
 * session list has this device. Rotation keeps the sign-in time, so
 * `fresh_until` minus it is the window.
 */
export function freshWindow(session: Pick<PublicSession, 'fresh_until'>, current: Pick<PublicSessionRow, 'signed_in_at'> | null): { at: string; minutes: number } | null {
  if (!current) return null;
  const signedIn = Date.parse(current.signed_in_at);
  const minutes = Math.round((Date.parse(session.fresh_until) - signedIn) / 60_000);
  if (!Number.isFinite(minutes) || minutes <= 0) return null;
  const at = new Date(signedIn).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  return { at, minutes };
}

/** The ownership write a press makes. */
export type OwnWrite =
  | { kind: 'hand'; timing: MemberTiming; to: Member }
  | { kind: 'ask'; timing: MemberTiming }
  | { kind: 'accept' | 'decline' | 'withdraw'; timing: MemberTiming; request: MemberOwnerRequest };

export function writePath(write: OwnWrite): string {
  if (write.kind === 'hand') return `/api/public/timings/${encodeURIComponent(write.timing.id)}/owner`;
  if (write.kind === 'ask') return `/api/public/timings/${encodeURIComponent(write.timing.id)}/owner-requests`;
  return `/api/public/owner-requests/${encodeURIComponent(write.request.id)}/${write.kind}`;
}

const VERBS: Record<OwnWrite['kind'], string> = { hand: 'hand it over', ask: 'ask', accept: 'accept', decline: 'decline', withdraw: 'withdraw' };

/** Who the write makes the owner; null when the owner stays. */
export const newOwner = (write: OwnWrite): Member | null => (write.kind === 'hand' ? write.to : write.kind === 'accept' ? write.request.requester : null);

/** What a done write changed: the owner as Discord posts it, or the ask's fate. */
export function doneWords(write: OwnWrite): string {
  const what = `${tokenWords(write.timing)} · ${whenWords(write.timing)}`;
  const owner = newOwner(write);
  if (owner) return `👑 ${owner.name} now owns weekly timing ${what}.`;
  if (write.kind === 'ask') return `Asked ${write.timing.owner.name} to own ${what}. The ask expires in 24 h.`;
  if (write.kind === 'withdraw') return `Withdrew your ask to own ${what}.`;
  return write.kind === 'decline' ? `Declined ${write.request.requester.name}'s ask to own ${what}.` : '';
}

/**
 * "Couldn't accept: Tsubame is no longer on the party. Ownership only moves
 * between members of the party." The server's sentence says the rule; the
 * front names who when it is about the would-be owner.
 */
export function refusalWords(write: OwnWrite, error: unknown): string {
  const code = error instanceof ApiRequestError ? error.body?.error : undefined;
  const message = error instanceof Error ? error.message : 'try again.';
  const owner = newOwner(write);
  const who = code === 'not_on_party' && owner ? `${owner.name} is no longer on the party. ` : '';
  return `Couldn't ${VERBS[write.kind]}: ${who}${message}`;
}
