// Relative times for the Reminders table (B_Reminders "In" column, the footer's
// "Next in 9 h" and the day groups' "today"), on the server's clock (MUST 8):
// the list's `generated_at` plus the monotonic time since it arrived, against
// each row's exact `fire_at`. A skewed browser clock changes nothing.

import { wallMinutes } from '../shared/wall';

const MINUTE = 60_000;

/** The day part of a row's `at`: "Tue 29 Sep". */
export const dayOf = (at: string) => at.replace(/\s+\d{1,2}:\d{2}$/, '');

/**
 * The server's now in epoch ms: its `generatedAt` advanced by the monotonic
 * time (`performance.now()`) elapsed since the response arrived at `receivedAt`.
 * Null when `generatedAt` is unreadable.
 */
export function serverNow(generatedAt: string, receivedAt: number, monotonic: number): number | null {
  const at = Date.parse(generatedAt);
  return Number.isNaN(at) ? null : at + Math.max(0, monotonic - receivedAt);
}

/** Minutes from `now` (epoch ms) until `fireAt` (negative once past); null when unreadable. */
export function minutesUntil(fireAt: string, now: number): number | null {
  const at = Date.parse(fireAt);
  return Number.isNaN(at) ? null : (at - now) / MINUTE;
}

/** Calendar days from today to `fireAt`'s day on the guild's wall clock (0 = today). */
export function daysUntil(fireAt: string, now: number, zone: string): number | null {
  const at = wallMinutes(fireAt, zone);
  const today = wallMinutes(new Date(now).toISOString(), zone);
  if (at === null || today === null) return null;
  return Math.floor(at / 1440) - Math.floor(today / 1440);
}

/** "45 min", "9 h" (whole hours under a day) or "2 d" (calendar days); signless. */
export function span(fireAt: string, now: number, zone: string): string {
  const until = minutesUntil(fireAt, now);
  const days = daysUntil(fireAt, now, zone);
  if (until === null || days === null) return '';
  const m = Math.abs(until);
  if (m < 60) return `${Math.floor(m)} min`;
  if (m < 1440) return `${Math.floor(m / 60)} h`;
  return `${Math.abs(days)} d`;
}
