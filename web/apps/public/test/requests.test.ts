import { describe, expect, it } from 'vitest';
import type { Boss, MemberRequest, MemberRun, MemberTiming, MemberWeek } from '@kanade/api-types';
import {
  bodyOf,
  cleanNote,
  counterWords,
  draftProposal,
  draftReturn,
  EMPTY,
  headline,
  instantWords,
  keptSubject,
  limitOf,
  limitToast,
  needsWeeks,
  NOTE_MAX,
  prefill,
  proposalWords,
  rowTail,
  runWhen,
  stepsOf,
  subjectChoices,
  waitingWords,
  type Draft,
} from '../src/requests/form';

const boss = (token: string, name: string): Boss => ({ token, key: name, name, difficulty: 'h', level: null, portrait: null, portrait_sm: null, art: null }) as unknown as Boss;

const DOWS = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'];
const week = (starts: string, runs: MemberRun[]): MemberWeek => ({
  starts,
  timezone: 'Asia/Singapore',
  reset: 'Thu 08:00',
  days: DOWS.map((dow, index) => ({ index, dow, date: new Date(Date.parse(starts) + index * 86_400_000).toISOString().slice(0, 10), is_reset: index === 0, is_today: false })),
  runs,
  generated_at: '2026-10-10T04:00:00Z',
  version: 1,
});

const run = (id: string, day: number, time: string | null, mine: boolean, over: Partial<MemberRun> = {}): MemberRun => ({
  id,
  day,
  time,
  minutes: 60,
  status: 'planned',
  bosses: [boss('HLimbo', 'Limbo')],
  tally: { on: 3, total: 6 },
  participants: [],
  party: 'Limbo crew',
  channel: '#limbo-crew',
  fixed_id: null,
  mine,
  can_edit: mine,
  ...over,
});

const timing = (id: string, weekday: number, time: string): MemberTiming => ({
  id,
  bosses: [boss('XKalos', 'Kalos')],
  weekday,
  time,
  party: [
    { id: '1001', name: 'Asahi' },
    { id: '1002', name: 'Ren' },
  ],
  owner: { id: '1001', name: 'Asahi' },
  owner_pinned: false,
  you_own: true,
  requests: [],
});

const draft = (over: Partial<Draft>): Draft => ({ ...EMPTY, party: [], ...over });
const query = (text: string) => new URLSearchParams(text);

describe('request form prefill', () => {
  const mine = (id: string) => id === 'r-limbo';

  it('picks Leave for the member’s own run and Join for anyone else’s', () => {
    expect(prefill(query('run=r-limbo'), mine)).toMatchObject({ kind: 'leave', subject: 'run:r-limbo' });
    expect(prefill(query('run=r-jupiter'), mine)).toMatchObject({ kind: 'join', subject: 'run:r-jupiter' });
    expect(prefill(query('run=r-limbo&kind=swap'), mine)).toMatchObject({ kind: 'swap', subject: 'run:r-limbo' });
  });

  it('reads Discord’s weekly links: change=edit with its day and time, change=remove as leaving it', () => {
    expect(prefill(query('fixed=f-kalos&change=edit&day=thu&time=21:30'), mine)).toMatchObject({ kind: 'change_fixed', subject: 'fixed:f-kalos', day: 3, time: '21:30' });
    expect(prefill(query('fixed=f-kalos&change=edit&day=Friday'), mine)).toMatchObject({ day: 4, time: '' });
    expect(prefill(query('fixed=f-kalos&change=remove'), mine)).toMatchObject({ kind: 'leave', subject: 'fixed:f-kalos' });
  });

  it('drops what it cannot read and keeps the rest of a returned draft', () => {
    expect(prefill(query('kind=new_fixed&day=someday&time=25:00'), mine)).toMatchObject({ kind: 'new_fixed', subject: '', day: null, time: '' });
    expect(prefill(query('kind=bogus'), mine).kind).toBe(EMPTY.kind);
    expect(prefill(query('kind=new_fixed&party=1,2&bosses=HLimbo&channel=c-1&note=hi'), mine)).toMatchObject({ party: ['1', '2'], bosses: 'HLimbo', channel: 'c-1', note: 'hi' });
  });

  it('waits for the weeks only when a run’s kind depends on them', () => {
    expect(needsWeeks(query('run=r-1'))).toBe(true);
    expect(needsWeeks(query('run=r-1&kind=join'))).toBe(false);
    expect(needsWeeks(query('fixed=f-1&change=edit'))).toBe(false);
  });
});

describe('request body per kind', () => {
  it('join and leave name a run or a weekly timing', () => {
    expect(bodyOf(draft({ kind: 'join', subject: 'run:r-1' }))).toEqual({ body: { kind: 'join', run_id: 'r-1' } });
    expect(bodyOf(draft({ kind: 'leave', subject: 'fixed:f-1', note: '  away  ' }))).toEqual({ body: { kind: 'leave', fixed_id: 'f-1', note: 'away' } });
    expect(bodyOf(draft({ kind: 'leave' }))).toEqual({ missing: 'Pick a run.' });
  });

  it('swap needs who takes the place', () => {
    expect(bodyOf(draft({ kind: 'swap', subject: 'run:r-1' }))).toEqual({ missing: 'Pick who takes your place.' });
    expect(bodyOf(draft({ kind: 'swap', subject: 'run:r-1', with: '1002' }))).toEqual({ body: { kind: 'swap', run_id: 'r-1', with: '1002' } });
  });

  it('a weekly change sends only what changes, never bosses, and keeps the requester in a new party', () => {
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'run:r-1' }))).toEqual({ missing: 'Pick a weekly run.' });
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'fixed:f-1' }))).toEqual({ missing: 'Change the day, time, channel or party.' });
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'fixed:f-1', time: '9pm' }))).toEqual({ missing: 'Write the time as HH:MM.' });
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'fixed:f-1', day: 4, time: '21:00', bosses: 'HLimbo' }))).toEqual({ body: { kind: 'change_fixed', fixed_id: 'f-1', day: 4, time: '21:00' } });
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'fixed:f-1', party: ['1002'] }), '1001')).toEqual({ body: { kind: 'change_fixed', fixed_id: 'f-1', party: ['1001', '1002'] } });
    expect(bodyOf(draft({ kind: 'change_fixed', subject: 'fixed:f-1', channel: 'c-2', party: ['1001', '1002'] }), '1001')).toEqual({
      body: { kind: 'change_fixed', fixed_id: 'f-1', channel_id: 'c-2', party: ['1001', '1002'] },
    });
  });

  it('a new weekly run needs bosses, a day and time, and a channel', () => {
    expect(bodyOf(draft({ kind: 'new_fixed' }))).toEqual({ missing: 'Name the bosses, for example HLimbo.' });
    expect(bodyOf(draft({ kind: 'new_fixed', bosses: 'HLimbo' }))).toEqual({ missing: 'Pick a day and a time (HH:MM).' });
    expect(bodyOf(draft({ kind: 'new_fixed', bosses: 'HLimbo', day: 0, time: '21:00' }))).toEqual({ missing: 'Pick the party channel.' });
    expect(bodyOf(draft({ kind: 'new_fixed', bosses: 'HLimbo, HStar + NJupiter', day: 0, time: '21:00', channel: 'c-1', party: ['1002'] }))).toEqual({
      body: { kind: 'new_fixed', bosses: ['HLimbo', 'HStar', 'NJupiter'], day: 0, time: '21:00', channel_id: 'c-1', party: ['1002'] },
    });
  });

  it('the note is one trimmed line of at most 200 characters', () => {
    expect(NOTE_MAX).toBe(200);
    expect(cleanNote('  Something came up\non Sunday.\t ')).toBe('Something came up on Sunday.');
    expect(bodyOf(draft({ kind: 'join', subject: 'run:r-1', note: 'two\r\nlines' }))).toEqual({ body: { kind: 'join', run_id: 'r-1', note: 'two lines' } });
    expect(bodyOf(draft({ kind: 'join', subject: 'run:r-1', note: ` ${'x'.repeat(200)} ` }))).toMatchObject({ body: { note: 'x'.repeat(200) } });
    expect(bodyOf(draft({ kind: 'join', subject: 'run:r-1', note: 'x'.repeat(201) }))).toEqual({ missing: 'Keep the note to 200 characters.' });
  });
});

describe('the draft through a fresh sign-in', () => {
  it('comes back as the same draft', () => {
    const sent = draft({ kind: 'change_fixed', subject: 'fixed:f-kalos', day: 4, time: '21:00', channel: 'c-2', party: ['1002', '1005'], note: 'Fridays suit Ren better.' });
    const back = draftReturn(sent);
    expect(back.noteKept).toBe(true);
    expect(back.path.startsWith('/requests/new?')).toBe(true);
    expect(prefill(new URL(back.path, 'https://kanade.local').searchParams, () => false)).toEqual(sent);
    const swap = draft({ kind: 'swap', subject: 'run:r-1', with: '1002' });
    expect(prefill(new URL(draftReturn(swap).path, 'https://kanade.local').searchParams, () => true)).toEqual(swap);
  });

  it('leaves out a note too long to carry, and says so', () => {
    const long = draft({ kind: 'join', subject: 'run:r-1', note: 'ü'.repeat(200) });
    const back = draftReturn(long);
    expect(back.noteKept).toBe(false);
    expect(back.path).not.toContain('note=');
    expect(draftReturn(draft({ kind: 'join', subject: 'run:r-1' }))).toEqual({ path: '/requests/new?kind=join&run=r-1', noteKept: true });
  });
});

describe('which run each kind lists', () => {
  const weeks = {
    this: week('2026-10-08', [run('r-limbo', 3, '20:00', true), run('r-jupiter', 4, '21:00', false), run('r-done', 0, '20:00', true, { status: 'done' })]),
    next: week('2026-10-15', [run('n-carling', 0, '22:00', true)]),
  };
  const timings = [timing('f-kalos', 4, '22:00')];

  it('Join lists runs the member is not in; Leave and Swap their own, this week then next', () => {
    expect(subjectChoices('join', weeks, timings, '').map((c) => c.value)).toEqual(['run:r-jupiter']);
    const leave = subjectChoices('leave', weeks, timings, '');
    expect(leave.map((c) => [c.value, c.when, c.next])).toEqual([
      ['run:r-limbo', 'Sun 11 · 20:00', false],
      ['run:n-carling', 'Thu 15 · 22:00', true],
    ]);
    expect(leave[0]).toMatchObject({ title: 'HLimbo', long: 'Sun 11 Oct 20:00', channel: '#limbo-crew', blocked: '' });
  });

  it('keeps a linked run another kind must take, blocked with the way to ask', () => {
    const leave = subjectChoices('leave', weeks, timings, 'run:r-jupiter');
    expect(leave.find((c) => c.value === 'run:r-jupiter')?.blocked).toBe('not in this run · use Join');
    expect(subjectChoices('join', weeks, timings, 'run:r-limbo').find((c) => c.value === 'run:r-limbo')?.blocked).toBe("you're in this run · use Leave");
    expect(keptSubject(leave, 'run:r-jupiter')).toBe('');
    expect(keptSubject(leave, 'run:r-limbo')).toBe('run:r-limbo');
  });

  it('weekly kinds list weekly timings; a weekly timing a link named joins Leave', () => {
    expect(subjectChoices('change_fixed', weeks, timings, '').map((c) => [c.value, c.when])).toEqual([['fixed:f-kalos', 'every Fri 22:00']]);
    expect(subjectChoices('new_fixed', weeks, timings, '')).toEqual([]);
    expect(subjectChoices('leave', weeks, timings, 'fixed:f-kalos')[0]?.value).toBe('fixed:f-kalos');
    expect(subjectChoices('join', weeks, timings, 'fixed:f-kalos').some((c) => c.value === 'fixed:f-kalos')).toBe(false);
  });
});

describe('request words', () => {
  const TZ = 'Asia/Singapore';
  const NOW = '2026-10-10T04:00:00Z';

  it('headlines and when', () => {
    expect(headline('swap', 'HCarling + HStar')).toBe('Swap my place on HCarling + HStar');
    expect(headline('change_fixed', 'XKalos')).toBe('Change weekly XKalos');
    const w = week('2026-10-08', []);
    expect(runWhen({ day: 3, time: '20:00' }, w)).toBe('Sun 11 Oct 20:00');
    expect(runWhen({ day: 3, time: null }, w)).toBe('Sun 11 Oct');
  });

  it('what a weekly change proposes, from a sent request or a draft', () => {
    const kalos = timing('f-kalos', 4, '22:00');
    expect(proposalWords({ proposed: { day: null, time: '21:30', channel: null, party: null } }, kalos)).toBe('Fri 22:00 → 21:30');
    // Channel names arrive as Discord shows them, with the leading `#`.
    const options = { channels: [{ id: 'c-2', name: '#kalos-four' }], members: [{ id: '1002', name: 'Ren' }] };
    const proposed = draftProposal(draft({ day: 5, channel: 'c-2', party: ['1002'] }), options);
    expect(proposed).toEqual({ day: 5, time: null, channel: '#kalos-four', party: [{ id: '1002', name: 'Ren' }] });
    expect(proposalWords({ proposed }, null)).toBe('→ Sat · #kalos-four · Ren');
  });

  it('limits: the count, the reason the key is off and the toast', () => {
    const open = { open: 3, max_open: 3, today: 4, max_today: 6 };
    expect(counterWords({ open: 1, max_open: 3, today: 1, max_today: 6 })).toBe('1 of 3 open · 1 of 6 today');
    expect(limitOf({ open: 1, max_open: 3, today: 1, max_today: 6 })).toBeNull();
    expect(limitOf(open)).toMatchObject({ limit: 'open', lead: '3 requests are already waiting.' });
    expect(limitOf({ open: 1, max_open: 3, today: 6, max_today: 6 })?.limit).toBe('today');
    expect(limitToast('open', open)).toBe('Limit reached: 3 requests waiting. Nothing was sent.');
    expect(limitToast('today', open)).toBe('Limit reached: 6 requests today. Nothing was sent.');
  });

  it('the stepper names how a request ended', () => {
    expect(stepsOf('waiting')).toMatchObject({ last: 'todo', reviewed: false, label: 'Decided' });
    expect(stepsOf('approved')).toMatchObject({ last: 'done', reviewed: true, label: 'Approved' });
    expect(stepsOf('rejected')).toMatchObject({ last: 'done', label: 'Declined' });
    expect(stepsOf('expired')).toMatchObject({ last: 'gone', label: 'Expired, not decided' });
    expect(stepsOf('withdrawn')).toMatchObject({ last: 'gone', label: 'Withdrawn' });
  });

  it('times in the guild’s zone, against the server’s clock', () => {
    expect(instantWords('2026-10-10T03:02:00Z', TZ, NOW)).toBe('today 11:02');
    expect(instantWords('2026-09-30T11:40:00Z', TZ, NOW)).toBe('Wed 30 Sep 19:40');
    expect(instantWords('2026-10-03T01:00:00Z', TZ, NOW, false)).toBe('Sat 3 Oct');
    expect(instantWords('not a time', TZ, NOW)).toBe('not a time');
  });

  it('each row says who decided, why, or when it was sent', () => {
    const base: Pick<MemberRequest, 'state' | 'sent_at' | 'decided_by' | 'reason'> = { state: 'waiting', sent_at: '2026-10-10T03:02:00Z', decided_by: null, reason: null };
    expect(rowTail(base, TZ, NOW)).toBe('sent today 11:02');
    expect(rowTail({ ...base, state: 'approved', decided_by: 'Asahi' }, TZ, NOW)).toBe('by Asahi');
    expect(rowTail({ ...base, state: 'rejected', decided_by: 'Asahi', reason: 'party is full' }, TZ, NOW)).toBe('Asahi: “party is full”');
    expect(rowTail({ ...base, state: 'expired' }, TZ, NOW)).toBe('not decided in time');
    expect(rowTail({ ...base, state: 'withdrawn' }, TZ, NOW)).toBe('by you');
  });

  it('a waiting request says what holds until an admin decides', () => {
    expect(waitingWords('change_fixed', 'Fri 22:00', 'Wed 14 Oct 23:59')).toBe(
      'Waiting on the admins. The weekly timing stays Fri 22:00 until one decides, and Discord mentions you when they do. It expires Wed 14 Oct 23:59 if nobody decides.',
    );
    expect(waitingWords('leave', '', '')).toBe('Waiting on the admins. You stay in the party until one decides, and Discord mentions you when they do.');
  });
});
