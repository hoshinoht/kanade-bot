import { describe, expect, it } from 'vitest';
import type { WeekDay } from '@kanade/api-types';
import { clashText, dayCells, edgeDay, nextOpenDay, readTyped, stepTime, suggestions, type PickerRun } from '../src/move/picker';
import { clashes, fromMinutes, toMinutes, type TimedRun } from '../src/move/slot';

// The boards' boss week: Thu 01 – Wed 07, today Sun 04, reset Thu.
const days: WeekDay[] = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'].map((dow, index) => ({
  index,
  dow,
  date: `2026-10-0${index + 1}`,
  is_reset: index === 0,
  is_today: index === 3,
}));
const run = (id: string, day: number, time: string | null, members: string[], minutes = 30): PickerRun => ({ id, title: id, day, time, minutes, members });
// P_MoveStates' runs; HLimbo (Mika, Yuzu, Rin) is the one being moved.
const field = [
  run('NBellona', 3, null, ['kaito', 'minato']),
  run('HFA', 4, '20:00', ['hinata', 'nagi', 'rin']),
  run('HJupiter', 4, '21:00', ['yuzu', 'sora', 'hotaru']),
  run('HCarling', 5, '22:00', ['asahi', 'ren', 'mika', 'tsubame']),
  run('HLimbo', 5, '23:30', ['mika', 'yuzu', 'rin']),
  run('XKalos', 6, '21:00', ['asahi', 'ren', 'tsubame', 'yuzu'], 60),
  run('XBM', 6, '23:30', ['minato', 'kaito', 'rin']),
];
const limbo = field[4]!;
const own = { day: 5, time: '23:30' };
const others = (day: number) => field.filter((r) => r.day === day && r.id !== limbo.id).length;
const names = (id: string) => id.charAt(0).toUpperCase() + id.slice(1);

describe('day strip', () => {
  it('marks reset, today, past, the run\'s own day once another is picked, and dots for other runs', () => {
    const cells = dayCells(days, 6, own.day, others);
    expect(cells.map((c) => c.tag)).toEqual(['reset', '', '', 'today', '', 'from', '']);
    expect(cells.map((c) => c.past)).toEqual([true, true, true, false, false, false, false]);
    expect(cells[4]!.dots).toBe(2);
    // A tagged day shows its word, not dots.
    expect(cells[3]!.dots).toBe(0);
    expect(cells[6]!.label).toBe('Wed 07, 2 other runs');
    expect(cells[5]!.label).toBe("Tue 06, the run's day, 1 other run");
    expect(cells[0]!.label).toBe('Thu 01, reset day, past');
    expect(cells[3]!.label).toBe('Sun 04, today, 1 other run');
  });

  it('shows "from" only once another day is picked', () => {
    expect(dayCells(days, 5, own.day, others)[5]!.tag).toBe('');
  });

  it('caps the dots at three; the label keeps the count', () => {
    const cells = dayCells(days, 6, null, () => 5);
    expect(cells[6]!.dots).toBe(3);
    expect(cells[6]!.label).toContain('5 other runs');
  });

  it('has nothing past in a week without today (next week)', () => {
    const next = days.map((d) => ({ ...d, is_today: false }));
    expect(dayCells(next, 0, null, () => 0).some((c) => c.past)).toBe(false);
  });

  it('←/→ skip past days and stop at the ends; Home is today, End the last day', () => {
    const cells = dayCells(days, 5, own.day, others);
    expect(nextOpenDay(cells, 3, -1)).toBe(3);
    expect(nextOpenDay(cells, 5, 1)).toBe(6);
    expect(nextOpenDay(cells, 6, 1)).toBe(6);
    expect(edgeDay(cells, 'home')).toBe(3);
    expect(edgeDay(cells, 'end')).toBe(6);
  });
});

describe('time stepper', () => {
  it('steps and wraps at midnight in both directions', () => {
    expect(stepTime(21 * 60 + 30, 30)).toBe(22 * 60);
    expect(stepTime(23 * 60 + 45, 30)).toBe(15);
    expect(stepTime(10, -30)).toBe(23 * 60 + 40);
    expect(stepTime(23 * 60 + 30, 60)).toBe(30);
  });
});

describe('typed shortcut', () => {
  const cells = dayCells(days, 5, own.day, others);
  it('reads against the picked slot: a bare time keeps the day, a bare day keeps the time', () => {
    expect(readTyped('9:45pm', days, cells, own)).toEqual({ kind: 'ok', slot: { day: 5, time: '21:45' } });
    expect(readTyped('wed', days, cells, own)).toEqual({ kind: 'ok', slot: { day: 6, time: '23:30' } });
    expect(readTyped('wed 21:30', days, cells, { day: 4, time: '20:00' })).toEqual({ kind: 'ok', slot: { day: 6, time: '21:30' } });
  });

  it('refuses a day that has passed, naming today', () => {
    expect(readTyped('sat 21:30', days, cells, own)).toEqual({ kind: 'error', message: 'Sat 03 has passed. Pick Sun 04 (today) or later.' });
  });

  it("keeps parseWhen's reasons for what it cannot read", () => {
    const read = readTyped('wedn 2130x', days, cells, own);
    expect(read.kind).toBe('error');
  });
});

describe('suggestions', () => {
  it('from the picked day: same time, then after and before each run on the step grid', () => {
    expect(suggestions(limbo, own, 6, field, 30)).toEqual([
      { label: 'same time', time: '23:30' },
      { label: 'after XKalos', time: '22:00' },
      { label: 'before XKalos', time: '20:30' },
    ]);
  });

  it('on its own day: no "same time"; times outside the day are dropped', () => {
    expect(suggestions(limbo, own, 5, field, 30)).toEqual([
      { label: 'after HCarling', time: '22:30' },
      { label: 'before HCarling', time: '21:30' },
    ]);
  });

  it('follows the Run lengths step', () => {
    expect(suggestions(limbo, own, 4, field, 15).map((s) => s.time)).toEqual(['23:30', '20:30', '19:30']);
  });
});

describe('clash wording', () => {
  it('names who is double-booked where, as the planner does', () => {
    expect(clashText(limbo, { day: 6, time: '23:30' }, field, names)).toBe('Rin in XBM 23:30');
    expect(clashText(limbo, { day: 6, time: '21:30' }, field, names)).toBe('Yuzu in XKalos 21:00');
  });

  it('needs a shared member, and is silent for own time', () => {
    expect(clashText(limbo, { day: 5, time: '22:00' }, field, names)).toBe('Mika in HCarling 22:00');
    expect(clashText({ ...limbo, members: ['nobody'] }, { day: 5, time: '22:00' }, field, names)).toBeNull();
    expect(clashText(limbo, { day: 6, time: null }, field, names)).toBeNull();
  });
});

describe('slot clock', () => {
  it('round-trips clock arithmetic', () => {
    expect(toMinutes('21:30')).toBe(1290);
    expect(fromMinutes(1290)).toBe('21:30');
    expect(fromMinutes(0)).toBe('00:00');
  });
});

const timed = (id: string, time: string | null, minutes: number, extra: Partial<TimedRun> = {}): TimedRun => ({ id, day: 4, time, minutes, ...extra });

describe('clashes', () => {
  const a = timed('a', '21:00', 60, { members: ['asahi', 'ren'] });

  it('reports an overlap only when a member is on both', () => {
    const other = timed('b', '21:30', 30, { members: ['ren', 'mika'] });
    expect(clashes(a, [a, other])).toEqual([{ with: other, members: ['ren'] }]);
  });

  it('allows overlaps without a shared member', () => {
    expect(clashes(a, [timed('b', '21:30', 30, { members: ['mika'] })])).toEqual([]);
  });

  it('back-to-back runs do not overlap', () => {
    expect(clashes(a, [timed('b', '22:00', 30, { members: ['asahi'] })])).toEqual([]);
    expect(clashes(a, [timed('b', '20:00', 60, { members: ['asahi'] })])).toEqual([]);
  });

  it('counts an overlap across midnight into the next day', () => {
    const late = timed('late', '23:30', 60, { members: ['asahi'] });
    expect(clashes(late, [timed('next', '00:00', 30, { day: 5, members: ['asahi'] })])).toHaveLength(1);
  });

  it('own-time runs and runs on other days never clash', () => {
    expect(clashes(a, [timed('own', null, 30, { members: ['asahi'] }), timed('b', '21:00', 30, { day: 2, members: ['asahi'] })])).toEqual([]);
    expect(clashes(timed('own', null, 30, { members: ['asahi'] }), [a])).toEqual([]);
  });
});
