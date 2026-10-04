// Relative times for the Reminders table (B_Reminders "In" column, the footer's
// "Next in 9 h" and the day groups' "today"). A row's `at` is the guild's wall
// clock ("Tue 29 Sep 21:00", no year); it is compared with now on the same
// wall clock, the year taken as the one nearest to now.

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const AT = /^\w{3} (\d{1,2}) (\w{3}) (\d{1,2}):(\d{2})$/;
const MINUTE = 60_000;

/** The day part of a row's `at`: "Tue 29 Sep". */
export const dayOf = (at: string) => at.replace(/\s+\d{1,2}:\d{2}$/, '');

/** `now` on the wall clock of `zone`, as minutes since the Unix epoch (zone offset folded in). */
function wallNow(now: Date, zone: string | undefined): { minutes: number; year: number } {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat('en-GB', { timeZone: zone, year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: 'numeric', hourCycle: 'h23' })
      .formatToParts(now)
      .map((p) => [p.type, Number(p.value)]),
  ) as Record<string, number>;
  return { minutes: Date.UTC(parts.year!, parts.month! - 1, parts.day!, parts.hour!, parts.minute!) / MINUTE, year: parts.year! };
}

/** Wall-clock minutes from now until `at` (negative once past); null for an unreadable `at`. */
export function minutesUntil(at: string, now: Date, zone: string | undefined): number | null {
  const match = AT.exec(at.trim());
  const month = match ? MONTHS.indexOf(match[2]!) : -1;
  if (!match || month < 0) return null;
  const wall = wallNow(now, zone);
  const [day, hour, minute] = [Number(match[1]), Number(match[3]), Number(match[4])];
  const diffs = [wall.year - 1, wall.year, wall.year + 1].map((y) => Date.UTC(y, month, day, hour, minute) / MINUTE - wall.minutes);
  return diffs.reduce((best, d) => (Math.abs(d) < Math.abs(best) ? d : best));
}

/** Calendar days from today to `at`'s day on the same wall clock (0 = today). */
export function daysUntil(at: string, now: Date, zone: string | undefined): number | null {
  const until = minutesUntil(at, now, zone);
  if (until === null) return null;
  const today = Math.floor(wallNow(now, zone).minutes / 1440);
  return Math.floor((wallNow(now, zone).minutes + until) / 1440) - today;
}

/** "45 min", "9 h" (whole hours under a day) or "2 d" (calendar days); signless. */
export function span(at: string, now: Date, zone: string | undefined): string {
  const until = minutesUntil(at, now, zone);
  const days = daysUntil(at, now, zone);
  if (until === null || days === null) return '';
  const m = Math.abs(until);
  if (m < 60) return `${Math.floor(m)} min`;
  if (m < 1440) return `${Math.floor(m / 60)} h`;
  return `${Math.abs(days)} d`;
}
