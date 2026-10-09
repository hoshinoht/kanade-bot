// The Week's progress bars (user decision 2026-10-04): how far the boss week
// has run, and how close a run is. "Now" is the server's clock
// (`generated_at`, refreshed by every poll), read on the guild's wall clock.
// Shared by the admin Week and the member portal (any week shape with these fields).
import type { RunStatus, WeekDay } from '@kanade/api-types';
import { dateMinutes, spanWords, wallMinutes } from './wall';

const DAY = 24 * 60;

export interface Progress {
  value: number;
  max: number;
  /** What the bar says, also its `aria-valuetext`. */
  text: string;
}

interface Clocked {
  days: WeekDay[];
  generated_at: string;
  timezone: string;
}

/**
 * "Day 5 of 7 · resets Thu 00:00", filled to the minute since the week's
 * reset; null for a week that is not running (next week).
 */
export function weekProgress(week: Clocked & { reset: string }): Progress | null {
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

/** The countdown waves over the last day before a run. */
export const COUNTDOWN_SPAN = DAY;
/** It zooms in stages (user decision 2026-10-04): the day down to T-1h, then the last hour. */
export const COUNTDOWN_STAGES = [DAY, 60];
/** Reminder marks, in minutes before the start. */
export const COUNTDOWN_MARKS = [60, 15];

export interface Countdown extends Progress {
  /** Minutes until the start. */
  left: number;
  /** Inside the final 24 h: the bar waves. */
  wavy: boolean;
  /** The reminder marks inside the current stage, as fractions of the bar. */
  ticks: number[];
}

/**
 * A run still ahead with a clock time: the bar fills from 24 h out to T-1h
 * (flat and empty before that), then restarts over the last hour with a
 * T-15m mark. Null for own-time, finished, cancelled and started runs.
 */
export function runCountdown(run: { day: number; time: string | null; status: RunStatus }, week: Clocked): Countdown | null {
  if (run.time === null || run.status === 'otot' || run.status === 'done' || run.status === 'cancelled') return null;
  const day = week.days[run.day];
  const start = day ? dateMinutes(day.date, run.time) : null;
  const now = wallMinutes(week.generated_at, week.timezone);
  if (start === null || now === null || start <= now) return null;
  const left = start - now;
  const span = COUNTDOWN_STAGES.findLast((s) => s >= left) ?? COUNTDOWN_SPAN;
  const end = COUNTDOWN_STAGES.find((s) => s < span) ?? 0;
  const max = span - end;
  return {
    left,
    value: Math.min(max, Math.max(0, span - left)),
    max,
    wavy: left <= COUNTDOWN_SPAN,
    ticks: COUNTDOWN_MARKS.filter((m) => m > end && m < span).map((m) => (span - m) / max),
    text: `starts in ${spanWords(left)}`,
  };
}
