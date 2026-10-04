import { describe as suite, expect, it } from 'vitest';
import type { ChangeRecord } from '@kanade/api-types';
import { actorName, describe, localAt, relativeAt, reminderKind, weekDate } from '../src/history/describe';
import { fieldChanges, fieldLabel, fieldValue, inRun } from '../src/sheet/runLog';

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

suite('run change log', () => {
  const ctx = { names, timeZone: TZ, channel: { id: '900', name: 'boss-xkalos' } };

  it('lists what a record changed on this run only, field by field', () => {
    const changes = fieldChanges(
      record([
        { key: { table: 'runs', id: 'r' }, before: run({}), after: run({ datetime: '2026-09-25T14:00:00+00:00', status: 'at_risk' }) },
        // The other run of a swap is not this run's change.
        { key: { table: 'runs', id: 'other' }, before: run({ id: 'other' }), after: run({ id: 'other', status: 'done' }) },
        { key: { table: 'rsvps', run_id: 'r', user_id: '1005' }, before: null, after: { run_id: 'r', user_id: '1005', state: 'no' } },
        { key: { table: 'reminders', id: 'rem-1' }, before: reminder({}), after: reminder({ fire_at: '2026-09-25T13:00:00+00:00' }) },
      ]),
      'r',
    );
    expect(changes.map((c) => c.field)).toEqual(['slot', 'status', 'rsvp:1005']);
    const [slot, status, rsvp] = changes;
    expect(`${fieldValue('slot', slot!.before, ctx)} → ${fieldValue('slot', slot!.after, ctx)}`).toBe('Fri 25 21:30 → Fri 25 22:00');
    expect(fieldValue('status', status!.after, ctx)).toBe('at risk');
    expect(`${fieldValue(rsvp!.field, rsvp!.before, ctx)} → ${fieldValue(rsvp!.field, rsvp!.after, ctx)}`).toBe('no answer → out');
  });

  it('names creation, roster, channel and attendance', () => {
    expect(fieldChanges(record([{ key: { table: 'runs', id: 'r' }, before: null, after: run({}) }]), 'r').map((c) => c.field)).toEqual(['created']);
    const changes = fieldChanges(
      record([
        {
          key: { table: 'runs', id: 'r' },
          before: run({}),
          after: run({ participants: ['1005', '1013'], channel_id: '901', attendance: [{ user_id: '1013', attended: true }] }),
        },
      ]),
      'r',
    );
    expect(changes.map((c) => c.field)).toEqual(['participants', 'channel', 'attended:1013']);
    expect(fieldValue('participants', changes[0]!.after, ctx)).toBe('Tsubame, Ren');
    expect(fieldValue('channel', changes[1]!.before, ctx)).toBe('boss-xkalos');
    expect(fieldValue('attended:1013', changes[2]!.after, ctx)).toBe('attended');
  });

  it('maps the domain field names to people words, and keeps unknown ones', () => {
    expect(fieldLabel('slot', names)).toBe('Day and time');
    expect(fieldLabel('participants', names)).toBe('Roster');
    expect(fieldLabel('status_pin', names)).toBe('Held status');
    expect(fieldLabel('rsvp:1005', names)).toBe("Tsubame's answer");
    expect(fieldLabel('attended:1013', names)).toBe("Ren's attendance");
    expect(fieldLabel('standing:1005', names)).toBe('standing:1005');
  });

  it('says how long ago by the server clock', () => {
    const now = '2026-09-29T04:00:00+00:00';
    expect(relativeAt('2026-09-29T03:59:40+00:00', now)).toBe('just now');
    expect(relativeAt('2026-09-29T03:48:00+00:00', now)).toBe('12 min ago');
    expect(relativeAt('2026-09-29T01:00:00+00:00', now)).toBe('3 h ago');
    expect(relativeAt('2026-09-26T04:00:00+00:00', now)).toBe('3 d ago');
    expect(relativeAt('2026-08-29T04:00:00+00:00', now)).toBe('4 wk ago');
  });

  it('drops the run title a line repeats, but keeps a boss change whole', () => {
    expect(inRun('XKalos: Fri 25 21:30 → Fri 25 22:00', 'XKalos')).toBe('Fri 25 21:30 → Fri 25 22:00');
    expect(inRun('XKalos roster: +Ren', 'XKalos')).toBe('Roster: +Ren');
    expect(inRun('XKalos → HStar', 'XKalos')).toBe('XKalos → HStar');
    expect(inRun('Tsubame → out on XKalos', 'XKalos')).toBe('Tsubame → out on XKalos');
  });

  it('names the run in its own log when the record carries no run row', () => {
    const answer = record([{ key: { table: 'rsvps', run_id: 'r', user_id: '1005' }, before: null, after: { run_id: 'r', user_id: '1005', state: 'no' } }]);
    expect(describe(answer, names, TZ, () => 'XKalos')).toEqual(['Tsubame → out on XKalos']);
  });
});
