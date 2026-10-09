import { describe, expect, it } from 'vitest';
import type { Run } from '@kanade/api-types';
import { openPlaces } from '@kanade/ui';
import { owedByMember } from '../src/week/waiting';

const run = (id: string, who: Run['participants'], status: Run['status'] = 'planned') =>
  ({ id, status, participants: who, tally: { on: who.filter((p) => p.answer === 'yes').length, total: who.length } }) as unknown as Run;

describe('still waiting, by member', () => {
  const runs = [
    run('a', [
      { id: 'm1', name: 'Mika', answer: 'waiting' },
      { id: 'm2', name: 'Ren', answer: 'maybe' },
      { id: 'm3', name: 'Sora', answer: 'yes' },
    ]),
    run('b', [{ id: 'm2', name: 'Ren', answer: 'waiting' }]),
    run('c', [{ id: 'm1', name: 'Mika', answer: 'waiting' }], 'done'),
    run('d', [{ id: 'm4', name: 'Asahi', answer: 'waiting' }]),
  ];

  it('counts waiting and maybe answers on live runs, most first, then by name', () => {
    const owed = owedByMember(runs, (r) => r.id.toUpperCase());
    expect(owed.map((o) => [o.name, o.runs.length])).toEqual([
      ['Ren', 2],
      ['Asahi', 1],
      ['Mika', 1],
    ]);
    expect(owed[0]!.runs).toEqual([
      { id: 'a', title: 'A', maybe: true },
      { id: 'b', title: 'B', maybe: false },
    ]);
  });

  it('leaves out yes and no answers and finished runs', () => {
    expect(owedByMember([runs[2]!], (r) => r.id)).toEqual([]);
  });
});

describe('open places', () => {
  it('reads the places still open, or full', () => {
    expect(openPlaces(run('x', [{ id: 'm1', name: 'Mika', answer: 'yes' }, { id: 'm2', name: 'Ren', answer: 'waiting' }]))).toBe('1 open');
    expect(openPlaces(run('y', [{ id: 'm1', name: 'Mika', answer: 'yes' }]))).toBe('full');
  });
});
