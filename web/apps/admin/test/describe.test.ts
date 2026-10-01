import { describe as suite, expect, it } from 'vitest';
import type { ChangeRecord } from '@kanade/api-types';
import { actorName, describe, localAt, reminderKind, weekDate } from '../src/history/describe';
import { blameField, blameValue } from '../src/sheet/blame';

const TZ = 'Asia/Kuala_Lumpur';
const names = (id: string) => ({ '1005': 'Tsubame', '1013': 'Ren' })[id] ?? id;
// Domain rows as kanade.change.v1 records carry them (instants in UTC).
const run = (over: Record<string, unknown>) => ({
  id: 'r',
  fixed_run_id: 'f',
  channel_id: '900',
  week_start: '2026-09-23T16:00:00+00:00',
  datetime: '2026-09-25T13:30:00+00:00',
  bosses: ['XKalos'],
  participants: ['1001', '1005'],
  status: 'planned',
  source: 'fixed',
  ...over,
});
const reminder = (over: Record<string, unknown>) => ({
  id: 'rem-1',
  run_id: 'r',
  kind: 'countdown_60',
  fire_at: '2026-09-25T12:30:00+00:00',
  sent_at: null,
  message_id: null,
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
    weeks: ['2026-09-23T16:00:00+00:00'],
    rows,
    notices: [],
    refs: [],
    prev_hash: '',
    hash: '',
  };
}

suite('history describe', () => {
  it('collapses identical lines (one roster edit across several weeks) into one with a count', () => {
    const edit = (id: string) => ({ key: { table: 'runs' as const, id }, before: run({ id }), after: run({ id, participants: ['1001', '1013'] }) });
    const lines = describe(record([edit('a'), edit('b'), edit('c')]), names, TZ);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(/ ×3$/);
  });

  it('says moves, status and roster changes in words, folding the reminders a move re-placed', () => {
    const lines = describe(
      record([
        { key: { table: 'runs', id: 'r' }, before: run({}), after: run({ datetime: '2026-09-25T14:00:00+00:00', status: 'at_risk', participants: ['1001', '1013'] }) },
        { key: { table: 'reminders', id: 'rem-1' }, before: reminder({}), after: reminder({ fire_at: '2026-09-25T13:00:00+00:00' }) },
      ]),
      names,
      TZ,
    );
    expect(lines).toEqual([
      'XKalos: Fri 25 21:30 → Fri 25 22:00',
      'XKalos: unconfirmed → at risk',
      'XKalos roster: +Ren −Tsubame',
      'XKalos: 1 reminder re-placed',
    ]);
  });

  it('names the run of an answer from the same record', () => {
    const lines = describe(
      record([
        { key: { table: 'runs', id: 'r' }, before: run({}), after: run({ status: 'at_risk' }) },
        { key: { table: 'rsvps', run_id: 'r', user_id: '1005' }, before: null, after: { run_id: 'r', user_id: '1005', state: 'no', source: 'chat', at: '' } },
      ]),
      names,
      TZ,
    );
    expect(lines[1]).toBe('Tsubame → out on XKalos');
  });

  it('describes reminder rows on their own in plain words', () => {
    const sent = describe(record([{ key: { table: 'reminders', id: 'rem-1' }, before: reminder({}), after: reminder({ sent_at: '2026-09-25T12:30:05+00:00' }) }]), names, TZ);
    expect(sent).toEqual(['T-1h card for run r sent Fri 25 20:30']);
    const added = describe(record([{ key: { table: 'reminders', id: 'rem-2' }, before: null, after: reminder({ kind: 'day_of', fire_at: '2026-09-25T01:00:00+00:00' }) }]), names, TZ);
    expect(added).toEqual(['Morning card for run r set for Fri 25 09:00']);
    expect(describe(record([{ key: { table: 'reminders', id: 'rem-1' }, before: reminder({}), after: null }]), names, TZ)).toEqual(['T-1h card for run r withdrawn']);
    expect(reminderKind('countdown_15')).toBe('T-15m card');
  });

  it('describes weekly timings from their domain rows', () => {
    const timing = { id: 'f', owner_id: '1005', channel_id: '900', bosses: ['XKalos'], weekday: 4, time: '21:30:00', participants: ['1005'], note: null };
    expect(describe(record([{ key: { table: 'fixed_runs', id: 'f' }, before: timing, after: { ...timing, time: '21:00:00' } }]), names, TZ)).toEqual([
      'Weekly timing XKalos: Friday 21:30 → Friday 21:00',
    ]);
    expect(describe(record([{ key: { table: 'fixed_runs', id: 'f' }, before: timing, after: null }]), names, TZ)).toEqual(['Weekly timing XKalos retired']);
  });

  it('reads record weeks as the instants they start', () => {
    expect(weekDate('2026-09-23T16:00:00+00:00', TZ)).toBe('2026-09-24');
    expect(weekDate('2026-09-24', TZ)).toBe('2026-09-24');
  });

  it('formats actors and guild-local instants', () => {
    expect(actorName({ kind: 'admin', id: 'token' }, names)).toBe('Admin (token)');
    const known = (id: string) => id === '1005';
    expect(actorName({ kind: 'admin', id: 'discord:1005' }, names, known)).toBe('Tsubame');
    expect(actorName({ kind: 'admin', id: 'discord:4242' }, names, known)).toBe('Admin (Discord 4242)');
    expect(actorName({ kind: 'admin', id: 'tailscale:ops@example.test' }, names)).toBe('Admin (ops@example.test)');
    expect(actorName({ kind: 'system', id: 'delivery' }, names)).toBe('system (delivery)');
    expect(localAt('2026-09-29T04:00:00+00:00', TZ)).toBe('Tue 29 Sep 12:00');
  });
});

suite('blame labels', () => {
  it('maps the domain field names to people words, and keeps unknown ones', () => {
    expect(blameField('slot', names)).toBe('Day and time');
    expect(blameField('participants', names)).toBe('Roster');
    expect(blameField('status_pin', names)).toBe('Held status');
    expect(blameField('rsvp:1005', names)).toBe("Tsubame's answer");
    expect(blameField('attended:1013', names)).toBe("Ren's attendance");
    expect(blameField('standing:1005', names)).toBe('standing:1005');
  });

  it('shows current values plainly', () => {
    expect(blameValue('slot', { datetime: '2026-09-25T14:00:00+00:00', week_start: '', source: 'amend', fixed_run_id: 'f' }, TZ)).toBe('Fri 25 22:00');
    expect(blameValue('rsvp:1005', { state: 'no' }, TZ)).toBe('out');
    expect(blameValue('rsvp:1005', null, TZ)).toBe('cleared');
    expect(blameValue('attended:1005', { user_id: '1005', attended: true }, TZ)).toBe('attended');
    expect(blameValue('status', 'at_risk', TZ)).toBe('at risk');
    expect(blameValue('participants', ['1', '2'], TZ)).toBe('2 people');
    expect(blameValue('bosses', ['XKalos', 'HStar'], TZ)).toBe('XKalos + HStar');
  });
});
