import { describe as suite, expect, it } from 'vitest';
import type { ChangeRecord } from '@kanade/api-types';
import { actorName, describe, localAt } from '../src/history/describe';

const names = (id: string) => ({ '1005': 'Tsubame', '1013': 'Ren' })[id] ?? id;
const run = (over: Record<string, unknown>) => ({
  week: '2026-09-24',
  day: 1,
  time: '21:30',
  status: 'planned',
  bosses: ['XKalos'],
  participants: ['1001', '1005'],
  ...over,
});

function record(rows: ChangeRecord['rows']): ChangeRecord {
  return {
    format: 'kanade.change.v1',
    seq: 3,
    id: 'x',
    revision: 4,
    at: '2026-09-25T09:00:00+00:00',
    actor: { kind: 'member', id: '1005' },
    surface: 'discord',
    request_id: null,
    weeks: ['2026-09-24'],
    rows,
    notices: [],
    refs: [],
    prev_hash: '',
    hash: '',
  };
}

suite('history describe', () => {
  it('says moves, status and roster changes in words', () => {
    const lines = describe(
      record([{ key: { table: 'runs', id: 'r' }, before: run({}), after: run({ time: '22:00', status: 'at_risk', participants: ['1001', '1013'] }) }]),
      names,
    );
    expect(lines).toEqual([
      'XKalos: Fri 25 21:30 → Fri 25 22:00',
      'XKalos: unconfirmed → at risk',
      'XKalos roster: +Ren −Tsubame',
    ]);
  });

  it('names the run of an answer from the same record', () => {
    const lines = describe(
      record([
        { key: { table: 'runs', id: 'r' }, before: run({}), after: run({ status: 'at_risk' }) },
        { key: { table: 'rsvps', run_id: 'r', user_id: '1005' }, before: null, after: { answer: 'no' } },
      ]),
      names,
    );
    expect(lines[1]).toBe('Tsubame → out on XKalos');
  });

  it('formats actors and guild-local instants', () => {
    expect(actorName({ kind: 'admin', id: 'admin-token' }, names)).toBe('admin token');
    expect(actorName({ kind: 'system', id: 'delivery' }, names)).toBe('system (delivery)');
    expect(localAt('2026-09-29T04:00:00+00:00', 'Asia/Kuala_Lumpur')).toBe('Tue 29 Sep 12:00');
  });
});
