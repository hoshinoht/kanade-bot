// The Week's progress bars (user decision 2026-10-04): how far the boss week
// has run, and how close a run is. "Now" is the server's clock
// (`Week.generated_at`, refreshed by every poll), read on the guild's wall clock.
import type { Run, Week } from '@kanade/api-types';
import { dateMinutes, spanWords, wallMinutes } from '../shared/wall';

const DAY = 24 * 60;

export interface Progress {
  value: number;
  max: number;
  /** What the bar says, also its `aria-valuetext`. */
  text: string;
}

type Clocked = Pick<Week, 'days' | 'reset' | 'generated_at' | 'timezone'>;

/**
 * "Day 5 of 7 · resets Thu 00:00", filled to the minute since the week's
 * reset; null for a week that is not running (next week).
 */
export function weekProgress(week: Clocked): Progress | null {
  const today = week.days.find((d) => d.is_today);
  const first = week.days[0];
  if (!today || !first) return null;
  const resetAt = /(\d{1,2}:\d{2})$/.exec(week.reset)?.[1] ?? '00:00';
  const start = dateMinutes(first.date, resetAt);
  const now = wallMinutes(week.generated_at, week.timezone);
  const max = week.days.length * DAY;
  const value = start === null || now === null ? 0 : Math.min(max, Math.max(0, now - start));
  return { value, max, text: `Day ${today.index + 1} of ${week.days.length} · resets ${week.reset}` };
}

/** The countdown spans the last day before a run; it waves only inside it. */
export const COUNTDOWN_SPAN = DAY;
/** Marks at the T-1h and T-15m reminders, in minutes before the start. */
export const COUNTDOWN_MARKS = [60, 15];

export interface Countdown extends Progress {
  /** Minutes until the start. */
  left: number;
  /** Inside the final 24 h: the bar waves. */
  wavy: boolean;
  /** The T-1h and T-15m marks as fractions of the bar. */
  ticks: number[];
}

/**
 * A run still ahead with a clock time: the bar fills over its final 24 h
 * (flat and empty before that), with marks at T-1h and T-15m. Null for
 * own-time, finished, cancelled and started runs.
 */
export function runCountdown(run: Pick<Run, 'day' | 'time' | 'status'>, week: Pick<Week, 'days' | 'generated_at' | 'timezone'>): Countdown | null {
  if (run.time === null || run.status === 'otot' || run.status === 'done' || run.status === 'cancelled') return null;
  const day = week.days[run.day];
  const start = day ? dateMinutes(day.date, run.time) : null;
  const now = wallMinutes(week.generated_at, week.timezone);
  if (start === null || now === null || start <= now) return null;
  const left = start - now;
  return {
    left,
    value: Math.max(0, COUNTDOWN_SPAN - left),
    max: COUNTDOWN_SPAN,
    wavy: left <= COUNTDOWN_SPAN,
    ticks: COUNTDOWN_MARKS.map((m) => (COUNTDOWN_SPAN - m) / COUNTDOWN_SPAN),
    text: `starts in ${spanWords(left)}`,
  };
}
