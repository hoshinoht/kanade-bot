import { describe, expect, it } from 'vitest';
import type { Run, Week } from '@kanade/api-types';
import { memberRuns, orderMembers, runCounts } from '../src/members/runs';

const day = (index: number, dow: string, is_today = false) => ({ index, date: `2026-09-${24 + index}`, dow, is_reset: index === 0, is_today });
const run = (id: string, dayIndex: number, time: string | null, who: Run['participants'], status: Run['status'] = 'planned') =>
  ({ id, day: dayIndex, time, status, bosses: [{ token: id.toUpperCase() }], participants: who }) as unknown as Run;

const week = {
  days: [day(0, 'Thu'), day(1, 'Fri'), day(2, 'Sat'), day(3, 'Sun'), day(4, 'Mon'), day(5, 'Tue', true), day(6, 'Wed')],
  runs: [
    run('late', 5, '22:00', [{ id: 'm1', name: 'Mika', answer: 'waiting' }]),
    run('other', 2, '20:00', [{ id: 'm2', name: 'Ren', answer: 'yes' }]),
    run('own', 4, null, [{ id: 'm1', name: 'Mika', answer: 'yes' }], 'done'),
    run('early', 4, '20:00', [{ id: 'm1', name: 'Mika', answer: 'yes' }], 'done'),
    run('fri', 1, '22:00', [{ id: 'm1', name: 'Mika', answer: 'yes' }], 'done'),
  ],
} as unknown as Week;

describe('a member’s runs this week', () => {
  it('lists only their runs, in week order with own time last on its day', () => {
    const { runs } = memberRuns(week, 'm1');
    expect(runs.map((r) => r.when)).toEqual(['Fri 22:00', 'Mon 20:00', 'Mon own time', 'Tue 22:00']);
    expect(runs.map((r) => r.answer)).toEqual(['yes', 'yes', 'yes', 'waiting']);
  });

  it('names the next run from today that is not done', () => {
    expect(memberRuns(week, 'm1').next?.title).toBe('LATE');
    expect(memberRuns(week, 'nobody')).toEqual({ runs: [], next: null });
  });
});

describe('roster order', () => {
  const rows = [
    { id: 'a', name: 'Yuzu' },
    { id: 'b', name: 'Asahi' },
    { id: 'c', name: 'Mika' },
    { id: 'd', name: 'Hinata' },
    { id: 'e', name: 'Kohane' },
  ];
  const counts = new Map([['a', 3], ['b', 4], ['c', 3], ['d', 3]]);
  const names = (list: typeof rows) => list.map((r) => r.name);

  it('puts the most runs first, A–Z between equals, and members with none last', () => {
    expect(names(orderMembers(rows, 'runs', (r) => r.name, (r) => counts.get(r.id) ?? 0))).toEqual(['Asahi', 'Hinata', 'Mika', 'Yuzu', 'Kohane']);
  });

  it('switches back to A–Z', () => {
    expect(names(orderMembers(rows, 'name', (r) => r.name, (r) => counts.get(r.id) ?? 0))).toEqual(['Asahi', 'Hinata', 'Kohane', 'Mika', 'Yuzu']);
  });

  it('counts every run of the week that lists the member', () => {
    expect(Object.fromEntries(runCounts(week))).toEqual({ m1: 4, m2: 1 });
  });
});
