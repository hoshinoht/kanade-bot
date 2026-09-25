/**
 * Keyboard alternative to dragging a run (WCAG 2.5.7): M on the focused card
 * picks up (Enter and Space still open it), arrows move (left/right = day,
 * up/down = 30 minutes), Enter/Space drops, Escape cancels. Pure so the
 * announcements are unit-tested.
 */

/** The pick-up shortcut; also exposed as `aria-keyshortcuts` on each card. */
export const PICK_KEY = 'M';

export interface Slot {
  day: number;
  time: string | null;
}

export type LiftState = { kind: 'idle' } | { kind: 'lifted'; runId: string; origin: Slot; at: Slot };

export interface MovableRun {
  id: string;
  day: number;
  time: string | null;
  label: string;
}

export interface MoveContext {
  dayLabel: (day: number) => string;
  lastDay: number;
}

export interface Outcome {
  state: LiftState;
  handled: boolean;
  announce?: string;
  commit?: { runId: string; from: Slot; to: Slot };
}

export const IDLE: LiftState = { kind: 'idle' };
export const STEP_MINUTES = 30;
const LAST_MINUTE = 23 * 60 + 59;

export function toMinutes(time: string): number {
  const [h = '0', m = '0'] = time.split(':');
  return Number(h) * 60 + Number(m);
}

export function fromMinutes(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
}

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

export function onKey(state: LiftState, key: string, run: MovableRun, ctx: MoveContext): Outcome {
  if (state.kind === 'idle' || state.runId !== run.id) {
    if (key.toUpperCase() !== PICK_KEY) return { state, handled: false };
    const origin = { day: run.day, time: run.time };
    return {
      state: { kind: 'lifted', runId: run.id, origin, at: origin },
      handled: true,
      announce:
        `Picked up ${run.label}, ${describeSlot(origin, ctx)}. ` +
        'Left and right arrows change the day, up and down change the time by 30 minutes. ' +
        'Enter or Space drops it, Escape cancels.',
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
      return { state: { ...state, at: next }, handled: true, announce: `${run.label}: ${describeSlot(next, ctx)}.` };
    }
    case 'ArrowUp':
    case 'ArrowDown': {
      if (at.time === null) return { state, handled: true, announce: 'Own-time runs have no time to change.' };
      const minutes = toMinutes(at.time) + (key === 'ArrowUp' ? -STEP_MINUTES : STEP_MINUTES);
      if (minutes < 0 || minutes > LAST_MINUTE) {
        return { state, handled: true, announce: `${at.time} is as ${key === 'ArrowUp' ? 'early' : 'late'} as this day goes.` };
      }
      const next = { ...at, time: fromMinutes(minutes) };
      return { state: { ...state, at: next }, handled: true, announce: `${run.label}: ${describeSlot(next, ctx)}.` };
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
    case 'Escape':
      return cancel(state, run, ctx);
    default:
      return { state, handled: false };
  }
}
