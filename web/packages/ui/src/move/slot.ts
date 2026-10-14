/**
 * A run's place in a boss week and the clash rule the admin planner and the
 * member portal share (user decisions 2026-10-01): an overlap that shares a
 * member is a clash; overlap alone is allowed. Pure.
 */

/** A run's day (boss-week index) and time (`HH:MM`, or null for own time). */
export interface Slot {
  day: number;
  time: string | null;
}

export interface TimedRun {
  id: string;
  day: number;
  /** `HH:MM`, or null for own time. */
  time: string | null;
  minutes: number;
  /** Member ids on the run; those who answered no are left out by the caller. */
  members?: string[];
}

const DAY = 24 * 60;

export function toMinutes(time: string): number {
  const [h = '0', m = '0'] = time.split(':');
  return Number(h) * 60 + Number(m);
}

export function fromMinutes(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
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
