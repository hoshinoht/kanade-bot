/**
 * "Edit, then approve" for Kanade's proposals (admin-api "Inbox (A6)"): the
 * typed day and time replace the proposed instant inside ITS boss week, which
 * may not be the week on screen. `day` counts from that week's reset day;
 * `time` is always `HH:MM`.
 */
import type { Proposal, Run, Week, WeekDay } from '@kanade/api-types';
import type { Slot } from '../planner/keyboardMove';
import { parseWhen } from '../sheet/parseWhen';

const DOWS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
/** `Fri 25 Sep 22:00`, the server's `when`. */
const WHEN = /^([A-Z][a-z]{2}) (\d{2}) ([A-Z][a-z]{2}) (\d{2}:\d{2})$/;

/** Only these carry a time the server lets an edit replace. */
export const editable = (p: Proposal) => p.tab === 'extractor' && ['move', 'add', 'split'].includes(p.kind) && !p.flags.includes('expired');

export type Edit = { ok: true; day: number; time: string } | { ok: false; message: string };

/** The proposal's boss week by weekday names only (its dates may not be on screen). */
function namedDays(resetDow: string): WeekDay[] | null {
  const reset = DOWS.indexOf(resetDow.slice(0, 3));
  if (reset < 0) return null;
  return Array.from({ length: 7 }, (_, index) => ({ index, dow: DOWS[(reset + index) % 7]!, date: '', is_reset: index === 0, is_today: false }));
}

/** `resetDow` is the boss week's first day (`Thu` here), from the week on screen. */
export function parseEdit(text: string, p: Proposal, resetDow: string): Edit {
  const proposed = WHEN.exec(p.when);
  const days = namedDays(resetDow);
  if (!proposed || !days) return { ok: false, message: 'The proposed time cannot be read; approve it as it is or reject it.' };
  // The proposal's week by weekday names: the calendar dates do not matter.
  const day = days.findIndex((d) => d.dow === proposed[1]);
  const parsed = parseWhen(text, days, { day, time: proposed[4]! });
  if (!parsed.ok) return parsed;
  if (!parsed.slot.time) return { ok: false, message: 'Give a time, for example "21:30".' };
  return { ok: true, day: parsed.slot.day, time: parsed.slot.time };
}

export interface EditWeek {
  /** The week on screen when the proposal falls in it, else its weekdays without dates. */
  days: WeekDay[];
  /** That week's runs (dots, suggestions, clashes); none when it is not on screen. */
  runs: Run[];
  /** The proposed slot, where the pick starts. */
  slot: Slot;
  /** The run's slot now (the dashed "from" day), when there is one. */
  own: Slot | null;
}

/** Where `when` falls among `days`: by date when they carry dates, else by weekday. */
function slotIn(when: string | null, days: WeekDay[]): Slot | null {
  const m = when ? WHEN.exec(when) : null;
  if (!m) return null;
  const month = String(MONTHS.indexOf(m[3]!) + 1).padStart(2, '0');
  const day = days.find((d) => (d.date ? d.date.slice(5) === `${month}-${m[2]}` : d.dow === m[1]));
  return day ? { day: day.index, time: m[4]! } : null;
}

/** Today's date (`YYYY-MM-DD`) in the guild's time zone, from the server's clock. */
export function isoToday(now: string, timeZone: string): string {
  const at = new Date(now);
  if (Number.isNaN(at.getTime())) return '';
  return new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(at);
}

const DAY_MS = 24 * 60 * 60 * 1000;

/** The named week dated around `when` (its year the one nearest today), today marked. */
function dated(m: RegExpExecArray, days: WeekDay[], today: string): WeekDay[] {
  const month = MONTHS.indexOf(m[3]!);
  const date = Number(m[2]);
  const [ty, tm, td] = today.split('-').map(Number) as [number, number, number];
  const now = Date.UTC(ty, tm - 1, td);
  const year = [ty - 1, ty + 1].reduce((best, y) => (Math.abs(Date.UTC(y, month, date) - now) < Math.abs(Date.UTC(best, month, date) - now) ? y : best), ty);
  const at = days.findIndex((d) => d.dow === m[1]);
  const base = Date.UTC(year, month, date);
  return days.map((d) => {
    const iso = new Date(base + (d.index - at) * DAY_MS).toISOString().slice(0, 10);
    return { ...d, date: iso, is_today: iso === today };
  });
}

/**
 * The Move picker's week for a proposal, or null when its time cannot be
 * read: the week on screen when the proposal falls in it; otherwise its own
 * boss week dated from `today` (no other runs known), or by weekday names.
 */
export function editWeek(p: Proposal, week: Week | null, resetDow: string, today = ''): EditWeek | null {
  const onScreen = week ? slotIn(p.when, week.days) : null;
  if (week && onScreen) {
    const run = week.runs.find((r) => r.id === p.run_id);
    const own = run ? { day: run.day, time: run.status === 'otot' ? null : run.time } : slotIn(p.from_when, week.days);
    return { days: week.days, runs: week.runs, slot: onScreen, own };
  }
  const named = namedDays(resetDow);
  const m = WHEN.exec(p.when);
  if (!named || !m || named.findIndex((d) => d.dow === m[1]) < 0) return null;
  const days = /^\d{4}-\d{2}-\d{2}$/.test(today) && MONTHS.includes(m[3]!) ? dated(m, named, today) : named;
  const slot = slotIn(p.when, days);
  if (!slot) return null;
  return { days, runs: [], slot, own: slotIn(p.from_when, days) };
}

/** The picked slot as the text parseEdit reads back ("wed 22:30"). */
export function editText(slot: Slot, days: WeekDay[]): string {
  return `${(days[slot.day]?.dow ?? '').toLowerCase()} ${slot.time ?? ''}`.trim();
}
