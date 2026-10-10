/**
 * Keyboard alternative to dragging a run (WCAG 2.5.7): M on the focused card
 * picks up (Enter and Space still open it), arrows move (left/right = day,
 * up/down = the configured run length, `run_lengths.default_minutes`),
 * Shift+Up/Down jump to just after the previous run / just before the next
 * one that day, Enter/Space drops, S swaps with the run whose slot it is on,
 * Escape cancels. Pure so the announcements are unit-tested.
 */
import { fromMinutes, toMinutes, type Slot, type TimedRun } from '@kanade/ui';
import { FIRST_MINUTE, LAST_MINUTE, snapTime } from './dropTime';

/** The pick-up shortcut; also exposed as `aria-keyshortcuts` on each card. */
export const PICK_KEY = 'M';

export type LiftState = { kind: 'idle' } | { kind: 'lifted'; runId: string; origin: Slot; at: Slot };

export interface MovableRun {
  id: string;
  day: number;
  time: string | null;
  label: string;
  /** Its length (bosses' run lengths); used by the Shift jumps. */
  minutes?: number;
}

export interface MoveContext {
  dayLabel: (day: number) => string;
  lastDay: number;
  /** Up/Down step in minutes (Config → Run lengths default); 30 when absent. */
  step?: number;
  /** The other timed runs of a day, for Shift+Up/Down. */
  others?: (day: number) => TimedRun[];
  /** Another live run already on this slot (its swap target), if any. */
  occupant?: (slot: Slot, movingId: string) => { id: string; label: string } | null;
}

export interface Outcome {
  state: LiftState;
  handled: boolean;
  announce?: string;
  commit?: { runId: string; from: Slot; to: Slot };
  swap?: { runId: string; withId: string };
}

export const SWAP_KEY = 'S';

/** A step's announcement, plus the swap hint when another run is on that slot. */
function landed(run: MovableRun, at: Slot, ctx: MoveContext): string {
  const here = ctx.occupant?.(at, run.id);
  return `${run.label}: ${describeSlot(at, ctx)}.${here ? ` ${here.label} is here: S swaps with it, Enter drops beside it.` : ''}`;
}

export const IDLE: LiftState = { kind: 'idle' };
export const STEP_MINUTES = 30;

export function describeSlot(slot: Slot, ctx: MoveContext): string {
  return `${ctx.dayLabel(slot.day)}, ${slot.time ?? 'own time'}`;
}

function same(a: Slot, b: Slot): boolean {
  return a.day === b.day && a.time === b.time;
}

export function cancel(state: LiftState, run: MovableRun, ctx: MoveContext): Outcome {
  if (state.kind !== 'lifted') return { state, handled: false };
  return { state: IDLE, handled: true, announce: `Move cancelled. ${run.label} stays on ${describeSlot(state.origin, ctx)}.` };
}

export function onKey(state: LiftState, key: string, run: MovableRun, ctx: MoveContext, shift = false): Outcome {
  const step = ctx.step ?? STEP_MINUTES;
  if (state.kind === 'idle' || state.runId !== run.id) {
    if (key.toUpperCase() !== PICK_KEY) return { state, handled: false };
    const origin = { day: run.day, time: run.time };
    return {
      state: { kind: 'lifted', runId: run.id, origin, at: origin },
      handled: true,
      announce:
        `Picked up ${run.label}, ${describeSlot(origin, ctx)}. ` +
        `Left and right arrows change the day, up and down change the time by ${step} minutes; ` +
        'with Shift, up jumps to just after the run before and down to just before the run after. ' +
        'Enter or Space drops it; on another run, S swaps the two. Escape cancels.',
    };
  }

  const { at } = state;
  switch (key) {
    case 'ArrowLeft':
    case 'ArrowRight': {
      const day = at.day + (key === 'ArrowLeft' ? -1 : 1);
      if (day < 0 || day > ctx.lastDay) {
        const edge = day < 0 ? 'first' : 'last';
        return { state, handled: true, announce: `${ctx.dayLabel(at.day)} is the ${edge} day of the boss week.` };
      }
      const next = { ...at, day };
      return { state: { ...state, at: next }, handled: true, announce: landed(run, next, ctx) };
    }
    case 'ArrowUp':
    case 'ArrowDown': {
      if (at.time === null) return { state, handled: true, announce: 'Own-time runs have no time to change.' };
      const direction = key === 'ArrowUp' ? -1 : 1;
      if (shift) {
        const self: TimedRun = { id: run.id, day: at.day, time: at.time, minutes: run.minutes ?? step };
        const time = snapTime(self, at.time, ctx.others?.(at.day) ?? [], direction);
        if (time === null) {
          return { state, handled: true, announce: `No run ${direction < 0 ? 'before' : 'after'} ${at.time} on ${ctx.dayLabel(at.day)} to move next to.` };
        }
        const next = { ...at, time };
        return { state: { ...state, at: next }, handled: true, announce: landed(run, next, ctx) };
      }
      const minutes = toMinutes(at.time) + direction * step;
      if (minutes < FIRST_MINUTE || minutes > LAST_MINUTE) {
        return { state, handled: true, announce: `${at.time} is as ${key === 'ArrowUp' ? 'early' : 'late'} as this day goes.` };
      }
      const next = { ...at, time: fromMinutes(minutes) };
      return { state: { ...state, at: next }, handled: true, announce: landed(run, next, ctx) };
    }
    case 'Enter':
    case ' ': {
      if (same(at, state.origin)) {
        return { state: IDLE, handled: true, announce: `Dropped ${run.label} where it was. Nothing changed.` };
      }
      return {
        state: IDLE,
        handled: true,
        announce: `Dropped ${run.label} on ${describeSlot(at, ctx)}.`,
        commit: { runId: run.id, from: state.origin, to: at },
      };
    }
    case 's':
    case 'S': {
      const target = ctx.occupant?.(at, run.id);
      if (!target) {
        return { state, handled: true, announce: `Nothing to swap with on ${describeSlot(at, ctx)}: move onto another run's slot first.` };
      }
      return {
        state: IDLE,
        handled: true,
        announce: `Swapping ${run.label} with ${target.label}.`,
        swap: { runId: run.id, withId: target.id },
      };
    }
    case 'Escape':
      return cancel(state, run, ctx);
    default:
      return { state, handled: false };
  }
}
