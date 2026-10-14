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
import { fromMinutes, toMinutes, type TimedRun } from '@kanade/ui';

export const FIRST_MINUTE = 0;
export const LAST_MINUTE = 23 * 60 + 59;

export interface DropTime {
  time: string | null;
  /** Set when the computed start fell outside the day and was held at an edge. */
  held?: 'start' | 'end';
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

/**
 * A slot exchange (POST /runs/{id}/swap): each run takes the other's day and
 * time; when either is own-time, only the days change and both keep their
 * clocks (as the server does).
 */
export function swapSlots<R extends Pick<TimedRun, 'day' | 'time'>>(a: R, aOwnTime: boolean, b: R, bOwnTime: boolean) {
  const daysOnly = aOwnTime || bOwnTime;
  return {
    daysOnly,
    a: { day: b.day, time: daysOnly ? a.time : b.time },
    b: { day: a.day, time: daysOnly ? b.time : a.time },
  };
}

/** A card's box on the board, top to bottom, without the dragged one. */
export interface CardBox {
  id: string;
  top: number;
  height: number;
  /** Done and cancelled runs take no swaps: their middle is a between zone too. */
  swappable: boolean;
}

/**
 * Where a pointer at height `y` sits among a day's cards: on a card's middle
 * half (its top and bottom quarters stay "between") means swap with it;
 * anywhere else is a move to `index` (the cards above the pointer's height).
 */
export type Zone = { kind: 'swap'; id: string } | { kind: 'between'; index: number };

export const SWAP_BAND = 0.25;

export function zoneAt(y: number, cards: CardBox[]): Zone {
  for (const card of cards) {
    const top = card.top + card.height * SWAP_BAND;
    const bottom = card.top + card.height * (1 - SWAP_BAND);
    if (card.swappable && y >= top && y <= bottom) return { kind: 'swap', id: card.id };
  }
  const index = cards.findIndex((card) => y < card.top + card.height / 2);
  return { kind: 'between', index: index < 0 ? cards.length : index };
}
