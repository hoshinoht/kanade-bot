import { describe, expect, it } from 'vitest';
import type { TimedRun } from '@kanade/ui';
import { dropTime, snapTime, swapSlots, timedOthers, zoneAt } from '../src/planner/dropTime';

const run = (id: string, time: string | null, minutes: number, extra: Partial<TimedRun> = {}): TimedRun => ({ id, day: 4, time, minutes, ...extra });

describe('drop time', () => {
  // Mon: 20:00 HFA (30), 21:00 HJupiter (30), own-time NBellona.
  const day = [run('hfa', '20:00', 30), run('jupiter', '21:00', 30), run('bellona', null, 30)];
  const moving = run('carling', '22:00', 60);

  it('starts right after the run above (its start + its minutes)', () => {
    expect(dropTime(moving, day, 1)).toEqual({ time: '20:30' });
    expect(dropTime(moving, day, 2)).toEqual({ time: '21:30' });
  });

  it('uses the multi-boss length of the run above', () => {
    const above = [run('carling-star', '19:00', 90)];
    expect(dropTime(moving, above, 1)).toEqual({ time: '20:30' });
  });

  it('at the top of a day, ends right before the run below (below − own minutes)', () => {
    expect(dropTime(moving, day, 0)).toEqual({ time: '19:00' });
  });

  it('skips own-time runs (they have no time) when finding the run above', () => {
    expect(dropTime(moving, day, 3)).toEqual({ time: '21:30' });
  });

  it('keeps its time on an empty day, or one with only own-time runs', () => {
    expect(dropTime(moving, [], 0)).toEqual({ time: '22:00' });
    expect(dropTime(moving, [run('bellona', null, 30)], 1)).toEqual({ time: '22:00' });
  });

  it('own-time runs keep no time wherever they land', () => {
    expect(dropTime(run('own', null, 30), day, 1)).toEqual({ time: null });
  });

  it('holds a slot before 00:00 at 00:00 and one past 23:59 at 23:59, saying which edge', () => {
    expect(dropTime(moving, [run('early', '00:30', 30)], 0)).toEqual({ time: '00:00', held: 'start' });
    expect(dropTime(moving, [run('late', '23:30', 60)], 1)).toEqual({ time: '23:59', held: 'end' });
    expect(dropTime(moving, [run('fits', '23:00', 59)], 1)).toEqual({ time: '23:59' });
  });

  it('lists a day’s other timed runs, earliest first', () => {
    const all = [run('b', '21:00', 30), run('a', '20:00', 30), run('own', null, 30), run('self', '19:00', 30), run('other-day', '18:00', 30, { day: 5 })];
    expect(timedOthers(all, 4, 'self').map((r) => r.id)).toEqual(['a', 'b']);
  });
});

describe('Shift jumps', () => {
  const others = [run('hfa', '20:00', 30), run('jupiter', '21:00', 60)];
  const me = run('carling', '22:30', 60);

  it('Shift+Up lands just after an earlier run ends, one run at a time', () => {
    expect(snapTime(me, '22:30', others, -1)).toBe('22:00');
    expect(snapTime(me, '22:00', others, -1)).toBe('20:30');
    expect(snapTime(me, '20:30', others, -1)).toBeNull();
  });

  it('Shift+Up never jumps to before the previous run (only run ends count)', () => {
    // Right after HFA (20:00–20:30): nothing earlier ends, so it stays.
    expect(snapTime(run('x', '20:30', 30), '20:30', [run('hfa', '20:00', 30)], -1)).toBeNull();
  });

  it('Shift+Down lands just before a later run starts, one run at a time', () => {
    expect(snapTime(me, '18:00', others, 1)).toBe('19:00');
    expect(snapTime(me, '19:00', others, 1)).toBe('20:00');
    expect(snapTime(me, '20:00', others, 1)).toBeNull();
  });

  it('Shift+Down never jumps to after the next run (only run starts count)', () => {
    expect(snapTime(run('x', '19:30', 30), '19:30', [run('hfa', '20:00', 30)], 1)).toBeNull();
  });

  it('never offers a point outside the day', () => {
    expect(snapTime(run('x', '01:00', 120), '00:00', [run('a', '00:30', 30)], 1)).toBeNull();
    expect(snapTime(run('x', '23:00', 30), '23:00', [run('a', '22:30', 120)], -1)).toBeNull();
  });
});

describe('swap zones', () => {
  // Two cards: 100–180 and 190–270 (80 px tall; middle bands 120–160, 210–250).
  const cards = [
    { id: 'carling', top: 100, height: 80, swappable: true },
    { id: 'bm', top: 190, height: 80, swappable: true },
  ];

  it('a card’s middle half swaps with it', () => {
    expect(zoneAt(140, cards)).toEqual({ kind: 'swap', id: 'carling' });
    expect(zoneAt(120, cards)).toEqual({ kind: 'swap', id: 'carling' });
    expect(zoneAt(250, cards)).toEqual({ kind: 'swap', id: 'bm' });
  });

  it('its top and bottom quarters, and the gaps, are between cards', () => {
    expect(zoneAt(90, cards)).toEqual({ kind: 'between', index: 0 });
    expect(zoneAt(105, cards)).toEqual({ kind: 'between', index: 0 });
    expect(zoneAt(170, cards)).toEqual({ kind: 'between', index: 1 });
    expect(zoneAt(185, cards)).toEqual({ kind: 'between', index: 1 });
    expect(zoneAt(265, cards)).toEqual({ kind: 'between', index: 2 });
    expect(zoneAt(400, cards)).toEqual({ kind: 'between', index: 2 });
  });

  it('a finished or cancelled card takes no swap: its middle is between', () => {
    expect(zoneAt(130, [{ ...cards[0]!, swappable: false }, cards[1]!])).toEqual({ kind: 'between', index: 0 });
    expect(zoneAt(150, [{ ...cards[0]!, swappable: false }, cards[1]!])).toEqual({ kind: 'between', index: 1 });
  });

  it('an empty day is between, at the start', () => {
    expect(zoneAt(140, [])).toEqual({ kind: 'between', index: 0 });
  });
});

describe('swap slots', () => {
  const a = { day: 4, time: '20:00' };
  const b = { day: 5, time: '23:30' };

  it('exchanges day and time', () => {
    expect(swapSlots(a, false, b, false)).toEqual({ daysOnly: false, a: { day: 5, time: '23:30' }, b: { day: 4, time: '20:00' } });
  });

  it('with an own-time run, only the days change and both keep their clocks', () => {
    expect(swapSlots(a, false, { day: 5, time: null }, true)).toEqual({ daysOnly: true, a: { day: 5, time: '20:00' }, b: { day: 4, time: null } });
    expect(swapSlots({ day: 4, time: null }, true, b, false)).toEqual({ daysOnly: true, a: { day: 5, time: null }, b: { day: 4, time: '23:30' } });
  });
});
