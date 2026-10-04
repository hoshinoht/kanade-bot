/**
 * The Move picker's rules (boards P_MoveStates, Main): the boss week's day
 * strip, the time stepper's wrap, the typed shortcut read live through
 * parseWhen, suggestions from the picked day's runs and the planner's clash
 * wording. Pure, so every state is unit-tested.
 */
import type { Run, WeekDay } from '@kanade/api-types';
import { runTitle } from '@kanade/ui';
import { clashes, fromMinutes, toMinutes, type TimedRun } from '../planner/dropTime';
import type { Slot } from '../planner/keyboardMove';
import { parseWhen } from './parseWhen';

export const DAY_MINUTES = 24 * 60;
/** At most this many dots for a day's other runs; the label says the count. */
export const MAX_DOTS = 3;
export const MAX_SUGGESTIONS = 3;

/** A run as the picker weighs it: its slot, length and who plays (answer not no). */
export interface PickerRun extends TimedRun {
  title: string;
}

/** As the planner weighs it: own-time runs have no time, a no is not playing. */
export function pickerRun(run: Run): PickerRun {
  return {
    id: run.id,
    day: run.day,
    time: run.status === 'otot' ? null : run.time,
    minutes: run.minutes ?? 0,
    members: run.participants.filter((p) => p.answer !== 'no').map((p) => p.id),
    title: runTitle(run),
  };
}

/** The runs that can still clash: finished and cancelled ones are left out. */
export const liveRuns = (runs: Run[]): PickerRun[] => runs.filter((r) => r.status !== 'done' && r.status !== 'cancelled').map(pickerRun);

/** Member names as the runs carry them. */
export function namesIn(runs: Run[]): (id: string) => string {
  const names = new Map(runs.flatMap((r) => r.participants).map((p) => [p.id, p.name]));
  return (id) => names.get(id) ?? id;
}

export interface DayCell {
  index: number;
  dow: string;
  /** Two-digit day of the month; empty when the dates are unknown. */
  date: string;
  past: boolean;
  today: boolean;
  reset: boolean;
  /** The run's own day, once another day is picked. */
  from: boolean;
  others: number;
  dots: number;
  /** The one word under the date: reset, then today, then from. */
  tag: '' | 'reset' | 'today' | 'from';
  label: string;
}

export function dayCells(days: WeekDay[], selected: number, own: number | null, others: (day: number) => number): DayCell[] {
  const today = days.findIndex((d) => d.is_today);
  return days.map((d) => {
    const count = others(d.index);
    const past = today >= 0 && d.index < today;
    const from = own === d.index && selected !== d.index;
    const tag = d.is_reset ? 'reset' : d.is_today ? 'today' : from ? 'from' : '';
    const date = d.date ? d.date.slice(8, 10) : '';
    const bits = [date ? `${d.dow} ${date}` : d.dow];
    if (d.is_today) bits.push('today');
    if (d.is_reset) bits.push('reset day');
    if (own === d.index) bits.push("the run's day");
    if (count) bits.push(`${count} other run${count === 1 ? '' : 's'}`);
    if (past) bits.push('past');
    return {
      index: d.index,
      dow: d.dow,
      date,
      past,
      today: d.is_today,
      reset: d.is_reset,
      from,
      others: count,
      dots: tag ? 0 : Math.min(count, MAX_DOTS),
      tag,
      label: bits.join(', '),
    };
  });
}

/** The next open day from `from` in `direction`, skipping past ones; `from` when there is none. */
export function nextOpenDay(cells: Pick<DayCell, 'past'>[], from: number, direction: -1 | 1): number {
  for (let i = from + direction; i >= 0 && i < cells.length; i += direction) if (!cells[i]!.past) return i;
  return from;
}

/** Home: today (the first open day); End: the boss week's last open day. */
export function edgeDay(cells: Pick<DayCell, 'past' | 'today'>[], edge: 'home' | 'end'): number {
  if (edge === 'home') {
    const today = cells.findIndex((c) => c.today && !c.past);
    return today >= 0 ? today : Math.max(0, cells.findIndex((c) => !c.past));
  }
  for (let i = cells.length - 1; i >= 0; i -= 1) if (!cells[i]!.past) return i;
  return cells.length - 1;
}

/** One step of the time stepper: wraps at midnight and stays on the picked day. */
export function stepTime(minutes: number, delta: number): number {
  return (((minutes + delta) % DAY_MINUTES) + DAY_MINUTES) % DAY_MINUTES;
}

export type Typed = { kind: 'ok'; slot: Slot } | { kind: 'error'; message: string };

/**
 * The typed shortcut ("wed 21:30", "9:45pm"), read against the picked slot:
 * a bare time keeps the picked day, a bare day the picked time. A day that
 * has passed is refused with the first open day to pick instead.
 */
export function readTyped(text: string, days: WeekDay[], cells: DayCell[], picked: Slot): Typed {
  const parsed = parseWhen(text, days, picked);
  if (!parsed.ok) return { kind: 'error', message: parsed.message };
  const cell = cells[parsed.slot.day];
  if (cell?.past) {
    const open = cells[edgeDay(cells, 'home')];
    const name = (c: DayCell) => (c.date ? `${c.dow} ${c.date}` : c.dow);
    const pick = open && !open.past ? ` Pick ${name(open)}${open.today ? ' (today)' : ''} or later.` : '';
    return { kind: 'error', message: `${name(cell)} has passed.${pick}` };
  }
  return { kind: 'ok', slot: parsed.slot };
}

export interface Suggestion {
  label: string;
  time: string;
}

/**
 * From the picked day's runs: the run's own time on another day, then for
 * each timed run that day (earliest first) just after it ends and just
 * before it starts, on the step grid, inside the day; the first three.
 */
export function suggestions(run: PickerRun, own: Slot, day: number, field: PickerRun[], step: number): Suggestion[] {
  const grid = Math.max(1, step);
  const found: { label: string; at: number }[] = [];
  const add = (label: string, at: number) => {
    if (at < 0 || at >= DAY_MINUTES || found.some((s) => s.at === at)) return;
    found.push({ label, at });
  };
  if (day !== own.day && own.time !== null) add('same time', toMinutes(own.time));
  const others = field
    .filter((r) => r.id !== run.id && r.day === day && r.time !== null)
    .sort((a, b) => toMinutes(a.time!) - toMinutes(b.time!) || a.id.localeCompare(b.id));
  for (const other of others) {
    const start = toMinutes(other.time!);
    add(`after ${other.title}`, Math.ceil((start + other.minutes) / grid) * grid);
    add(`before ${other.title}`, Math.floor((start - run.minutes) / grid) * grid);
  }
  return found.slice(0, MAX_SUGGESTIONS).map((s) => ({ label: s.label, time: fromMinutes(s.at) }));
}

/**
 * The planner's clash wording ("Rin in XBM 23:30"; several joined by "; "),
 * or null when nobody is double-booked. Overlap alone is not a clash.
 */
export function clashText(run: PickerRun, slot: Slot, field: PickerRun[], names: (id: string) => string): string | null {
  const found = clashes({ ...run, day: slot.day, time: slot.time }, field);
  if (!found.length) return null;
  const titles = new Map(field.map((r) => [r.id, r.title]));
  return found.map((c) => `${c.members.map(names).join(', ')} in ${titles.get(c.with.id) ?? 'another run'} ${c.with.time}`).join('; ');
}
