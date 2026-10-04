import { describe, expect, it } from 'vitest';
import type { Proposal, Run, Week, WeekDay } from '@kanade/api-types';
import { clashText, dayCells, edgeDay, nextOpenDay, readTyped, stepTime, suggestions, type PickerRun } from '../src/sheet/move';
import { editText, editWeek, isoToday, parseEdit } from '../src/inbox/edit';

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

describe('Inbox edit week', () => {
  const week = { days, runs: [{ id: 'r-limbo', day: 5, time: '23:30', status: 'planned' } as Run], reset: 'Thu 00:00' } as Week;
  const proposal = (when: string, from: string | null = null) => ({ when, from_when: from, run_id: 'r-limbo' }) as Proposal;

  it('uses the week on screen when the proposal falls in it', () => {
    const at = editWeek(proposal('Wed 07 Oct 23:30'), week, 'Thu')!;
    expect(at.days).toBe(days);
    expect(at.slot).toEqual({ day: 6, time: '23:30' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
  });

  it('falls back to weekday names for another week', () => {
    const at = editWeek(proposal('Wed 14 Oct 21:00', 'Tue 13 Oct 23:30'), week, 'Thu')!;
    expect(at.days.map((d) => d.dow)).toEqual(['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed']);
    expect(at.days.every((d) => !d.date && !d.is_today)).toBe(true);
    expect(at.runs).toEqual([]);
    expect(at.slot).toEqual({ day: 6, time: '21:00' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
  });

  it('dates its own boss week from today when it is not on screen', () => {
    const today = isoToday('2026-09-29T04:00:00Z', 'Asia/Kuala_Lumpur');
    expect(today).toBe('2026-09-29');
    const at = editWeek(proposal('Wed 30 Sep 23:30', 'Tue 29 Sep 23:30'), null, 'Thu', today)!;
    expect(at.days.map((d) => d.date.slice(8))).toEqual(['24', '25', '26', '27', '28', '29', '30']);
    expect(at.days.findIndex((d) => d.is_today)).toBe(5);
    expect(at.slot).toEqual({ day: 6, time: '23:30' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
    // Across the new year, the year nearest today.
    const jan = editWeek(proposal('Fri 01 Jan 21:00'), null, 'Thu', '2026-12-30')!;
    expect(jan.days[0]!.date).toBe('2026-12-31');
  });

  it('round-trips the picked slot through parseEdit', () => {
    const p = { ...proposal('Wed 07 Oct 23:30'), when: 'Wed 07 Oct 23:30' } as Proposal;
    expect(parseEdit(editText({ day: 4, time: '22:30' }, days), p, 'Thu')).toEqual({ ok: true, day: 4, time: '22:30' });
  });
});
