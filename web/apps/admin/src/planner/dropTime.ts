/**
 * Where a dropped run lands in time, and whether it then clashes (user
 * decisions 2026-10-01, workplan step planner-time-drops). Pure, so the
 * pointer drop, the keyboard Shift jumps and the clash marks share one rule
 * and are unit-tested.
 *
 * - Dropped after a run: it starts when that run ends (start + minutes).
 * - Dropped at the top of a day: it ends when the run below starts.
 * - An empty day (or one with only own-time runs): it keeps its time.
 * - Own-time runs have no time and keep none: only the day changes.
 * - Times stay within the day, 00:00 to 23:59: a slot that would start
 *   earlier lands on 00:00, one that would start later on 23:59, and the
 *   result says which edge it was held at.
 */

export const FIRST_MINUTE = 0;
export const LAST_MINUTE = 23 * 60 + 59;
const DAY = 24 * 60;

export interface TimedRun {
  id: string;
  day: number;
  /** `HH:MM`, or null for own time. */
  time: string | null;
  minutes: number;
  /** Member ids on the run; those who answered no are left out by the caller. */
  members?: string[];
}

export interface DropTime {
  time: string | null;
  /** Set when the computed start fell outside the day and was held at an edge. */
  held?: 'start' | 'end';
}

export function toMinutes(time: string): number {
  const [h = '0', m = '0'] = time.split(':');
  return Number(h) * 60 + Number(m);
}

export function fromMinutes(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
}

function hold(minutes: number): DropTime {
  if (minutes < FIRST_MINUTE) return { time: fromMinutes(FIRST_MINUTE), held: 'start' };
  if (minutes > LAST_MINUTE) return { time: fromMinutes(LAST_MINUTE), held: 'end' };
  return { time: fromMinutes(minutes) };
}

/** Timed runs of a day, earliest first, without the one being moved. */
export function timedOthers(runs: TimedRun[], day: number, movingId: string): TimedRun[] {
  return runs
    .filter((r) => r.day === day && r.id !== movingId && r.time !== null)
    .sort((a, b) => toMinutes(a.time!) - toMinutes(b.time!) || a.id.localeCompare(b.id));
}

/**
 * The time a run gets when dropped into `dayRuns` (the target day's runs in
 * board order, without the moved run) before position `index`.
 */
export function dropTime(run: TimedRun, dayRuns: TimedRun[], index: number): DropTime {
  if (run.time === null) return { time: null };
  const above = dayRuns
    .slice(0, index)
    .filter((r) => r.time !== null)
    .at(-1);
  if (above) return hold(toMinutes(above.time!) + above.minutes);
  const below = dayRuns.slice(index).find((r) => r.time !== null);
  if (below) return hold(toMinutes(below.time!) - run.minutes);
  return { time: run.time };
}

/**
 * Shift+Up: the nearest end of a run that day (its start + minutes) earlier
 * than `at` -- just after the previous run. Shift+Down: the nearest start of
 * a later run minus this run's minutes, later than `at` -- just before the
 * next run. Repeated presses keep stepping that way; null when there is no
 * such run (or the slot would leave the day).
 */
export function snapTime(run: TimedRun, at: string, others: TimedRun[], direction: -1 | 1): string | null {
  const now = toMinutes(at);
  const points = others
    .filter((r) => r.time !== null)
    .map((r) => (direction < 0 ? toMinutes(r.time!) + r.minutes : toMinutes(r.time!) - run.minutes))
    .filter((m) => m >= FIRST_MINUTE && m <= LAST_MINUTE && (direction < 0 ? m < now : m > now));
  if (points.length === 0) return null;
  return fromMinutes(direction < 0 ? Math.max(...points) : Math.min(...points));
}

export interface Clash {
  /** The other run it overlaps. */
  with: TimedRun;
  /** Member ids on both. */
  members: string[];
}

/**
 * Runs `run` would overlap (by start + minutes, across midnight too) that
 * share a member. Overlap alone is allowed and not reported.
 */
export function clashes(run: TimedRun, runs: TimedRun[]): Clash[] {
  if (run.time === null || !run.members?.length) return [];
  const start = run.day * DAY + toMinutes(run.time);
  const end = start + Math.max(run.minutes, 1);
  const mine = new Set(run.members);
  const found: Clash[] = [];
  for (const other of runs) {
    if (other.id === run.id || other.time === null) continue;
    const otherStart = other.day * DAY + toMinutes(other.time);
    const otherEnd = otherStart + Math.max(other.minutes, 1);
    if (otherStart >= end || start >= otherEnd) continue;
    const shared = (other.members ?? []).filter((m) => mine.has(m));
    if (shared.length) found.push({ with: other, members: shared });
  }
  return found;
}
