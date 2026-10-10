// Calendar maths for the date picker. Days are whole numbers since 1970-01-01
// (a Thursday); dates are guild-local "YYYY-MM-DD". "Today" and boss weeks come
// from the server clock (`Week.generated_at`, `reset`, `timezone`), never the browser's.

export const DOWS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'] as const;
const DOWS_LONG = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const MONTHS_LONG = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
const DAY_MS = 86_400_000;

const at = (n: number) => new Date(n * DAY_MS);
const fromYmd = (y: number, m: number, d: number) => Math.round(Date.UTC(y, m, d) / DAY_MS);

/** Day of the week, 0 = Sunday. */
export const dow = (n: number) => (((n + 4) % 7) + 7) % 7;

export function isoOf(n: number): string {
  return at(n).toISOString().slice(0, 10);
}

/** A strict "YYYY-MM-DD" as a day; null when malformed, NaN when the day doesn't exist (2026-02-30). */
export function parseDay(text: string): number | null {
  const m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(text.trim());
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]) - 1, Number(m[3])];
  const n = fromYmd(y, mo, d);
  const back = at(n);
  return back.getUTCFullYear() === y && back.getUTCMonth() === mo && back.getUTCDate() === d ? n : NaN;
}

/** A stored ISO date as a day, or null when empty or unreadable. */
export function dayOf(iso: string): number | null {
  const n = parseDay(iso);
  return n === null || Number.isNaN(n) ? null : n;
}

/** "Thu 24 Sep". */
export function dayLabel(n: number): string {
  const x = at(n);
  return `${DOWS[x.getUTCDay()]} ${String(x.getUTCDate()).padStart(2, '0')} ${MONTHS[x.getUTCMonth()]}`;
}

/** "Thursday 24 September 2026", for a cell's accessible name. */
export function longLabel(n: number): string {
  const x = at(n);
  return `${DOWS_LONG[x.getUTCDay()]} ${x.getUTCDate()} ${MONTHS_LONG[x.getUTCMonth()]} ${x.getUTCFullYear()}`;
}

export function monthOf(n: number): { y: number; m: number } {
  const x = at(n);
  return { y: x.getUTCFullYear(), m: x.getUTCMonth() };
}

export const monthTitle = (y: number, m: number) => `${MONTHS_LONG[m]} ${y}`;
export const monthIndex = (y: number, m: number) => y * 12 + m;
export const firstOfMonth = (y: number, m: number) => fromYmd(y, m, 1);
export const dayOfMonth = (n: number) => at(n).getUTCDate();

/** The first day of the boss week holding `n`. */
export const weekStartOf = (n: number, firstDow: number) => n - ((dow(n) - firstDow + 7) % 7);

/**
 * Six boss-week rows for a month: the row holding the 1st, or the row before
 * it when the month starts on the reset day, so a range from the previous
 * month stays visible (P_Dates).
 */
export function monthCells(y: number, m: number, firstDow: number): number[] {
  const first = firstOfMonth(y, m);
  const lead = weekStartOf(first, firstDow);
  const start = lead === first ? first - 7 : lead;
  return Array.from({ length: 42 }, (_, i) => start + i);
}

/** Column heads from the reset day: THU … WED. */
export function weekHeads(firstDow: number): { short: string; long: string }[] {
  return Array.from({ length: 7 }, (_, i) => {
    const d = (firstDow + i) % 7;
    return { short: DOWS[d]!.toUpperCase(), long: DOWS_LONG[d]! + (i === 0 ? ' (reset)' : '') };
  });
}

export interface ServerClock {
  today: number;
  /** First day of the current boss week. */
  weekStart: number;
  /** The reset weekday, 0 = Sunday. */
  firstDow: number;
  timezone: string;
}

/** The server's "today" and boss week from what the week sent; null while unreadable. */
export function serverClock(week: { generated_at: string; timezone: string; reset: string } | null | undefined): ServerClock | null {
  if (!week) return null;
  const now = new Date(week.generated_at);
  if (Number.isNaN(now.getTime())) return null;
  const p: Record<string, string> = {};
  try {
    for (const part of new Intl.DateTimeFormat('en-US', {
      timeZone: week.timezone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hourCycle: 'h23',
    }).formatToParts(now))
      p[part.type] = part.value;
  } catch {
    return null;
  }
  const today = fromYmd(Number(p.year), Number(p.month) - 1, Number(p.day));
  const minute = Number(p.hour) * 60 + Number(p.minute);
  const reset = /^(\w{3})\w*\s+(\d{1,2}):(\d{2})/.exec(week.reset.trim());
  const found = reset ? DOWS.findIndex((d) => d.toLowerCase() === reset[1]!.toLowerCase()) : -1;
  const firstDow = found < 0 ? 4 : found;
  const resetMinute = reset ? Number(reset[2]) * 60 + Number(reset[3]) : 0;
  let weekStart = weekStartOf(today, firstDow);
  // Before the reset time on reset day, the previous boss week is still running.
  if (weekStart === today && minute < resetMinute) weekStart -= 7;
  return { today, weekStart, firstDow, timezone: week.timezone };
}

/** "Kuala Lumpur time" from "Asia/Kuala_Lumpur". */
export function zoneWords(timezone: string): string {
  const city = timezone.split('/').pop()?.replace(/_/g, ' ') ?? timezone;
  return /^(UTC|GMT|Etc)/.test(timezone) ? timezone : `${city} time`;
}

export interface RangeChip {
  label: string;
  from: number;
  to: number;
}

export function rangeChips(clock: ServerClock): RangeChip[] {
  const { today, weekStart } = clock;
  return [
    { label: 'This boss week', from: weekStart, to: today },
    { label: 'Last boss week', from: weekStart - 7, to: weekStart - 1 },
    { label: 'Last 7 days', from: today - 6, to: today },
  ];
}

export interface SinceChip {
  label: string;
  day: number;
}

export function sinceChips(clock: ServerClock): SinceChip[] {
  const { today, weekStart } = clock;
  const reset = `${DOWS[dow(weekStart)]} ${String(dayOfMonth(weekStart)).padStart(2, '0')}`;
  return [
    { label: `Last reset · ${reset}`, day: weekStart },
    { label: '7 days ago', day: today - 7 },
    { label: 'Last boss week', day: weekStart - 7 },
  ];
}

/** Earlier day first. */
export const ordered = (a: number, b: number): [number, number] => (a <= b ? [a, b] : [b, a]);

/** The range in words: one date when it is one day; open ends say so. */
export function rangeWords(from: number | null, to: number | null): string {
  if (from !== null && to !== null) return from === to ? dayLabel(from) : `${dayLabel(from)} – ${dayLabel(to)}`;
  if (from !== null) return `from ${dayLabel(from)}`;
  if (to !== null) return `until ${dayLabel(to)}`;
  return '';
}

export const dayCount = (from: number, to: number) => {
  const n = to - from + 1;
  return n === 1 ? '1 day' : `${n} days`;
};

export interface TypedError {
  message: string;
  /** The field to mark. */
  field: 'from' | 'to';
}

/** The typed From/To check (P_DatesSpec "Errors · typed fields"); null when the pair is good. */
export function typedError(fromText: string, toText: string, today: number): TypedError | null {
  const from = parseDay(fromText);
  const to = parseDay(toText);
  if (from === null || to === null) return { message: 'Use a date like 2026-09-28.', field: from === null ? 'from' : 'to' };
  if (Number.isNaN(from) || Number.isNaN(to)) return { message: 'That day doesn’t exist in that month.', field: Number.isNaN(from) ? 'from' : 'to' };
  if (to < from) return { message: `To (${dayLabel(to)}) is before From (${dayLabel(from)}).`, field: 'to' };
  if (from > today) return { message: `From is after today (${dayLabel(today)}).`, field: 'from' };
  if (to > today) return { message: `To is after today (${dayLabel(today)}).`, field: 'to' };
  return null;
}
