import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Answer, Boss, MemberRun, MemberRunLink, MemberWeek, RunStatus } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { MemberWeeks } from '../src/weeks.svelte';
import { RunWrites } from '../src/writes/runWrites.svelte';
import {
  answerable,
  answerReturn,
  askedWords,
  askPath,
  changeWords,
  instantOf,
  linkState,
  movable,
  moveChecks,
  moveReturn,
  returnedChoice,
  slotOf,
  weekEnds,
  weeklyAskPath,
  withAnswer,
} from '../src/writes/runs';

const ME = '42';

const boss = (token: string): Boss => ({ token, key: token.toLowerCase(), name: token, difficulty: 'hard', level: null, portrait: null, portrait_sm: null, art: null, animated: null, hue: 0 });

function run(id: string, day: number, time: string | null, options: { mine?: boolean; answer?: Answer; status?: RunStatus; canEdit?: boolean; others?: { id: string; name: string; answer: Answer }[] } = {}): MemberRun {
  const mine = options.mine ?? false;
  const participants = [
    ...(options.others ?? [
      { id: '7', name: 'Ren', answer: 'yes' as Answer },
      { id: '9', name: 'Mio', answer: 'waiting' as Answer },
    ]),
    ...(mine ? [{ id: ME, name: 'Asahi', answer: options.answer ?? ('yes' as Answer) }] : []),
  ];
  return {
    id,
    day,
    time,
    minutes: 60,
    status: options.status ?? 'planned',
    bosses: [boss(id.toUpperCase())],
    tally: { on: participants.filter((p) => p.answer === 'yes').length, total: participants.length },
    participants,
    party: '',
    channel: 'crew',
    fixed_id: null,
    mine,
    can_edit: options.canEdit ?? true,
  };
}

/** Thu 24 Sep 2026 (reset) … Wed 30 Sep; the server's clock is Mon 28 Sep 12:00 in Singapore. */
function week(runs: MemberRun[], version = 1): MemberWeek {
  const dows = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'];
  return {
    starts: '2026-09-24',
    timezone: 'Asia/Singapore',
    reset: 'Thu 00:00',
    days: dows.map((dow, index) => ({ index, date: `2026-09-${String(24 + index).padStart(2, '0')}`, dow, is_reset: index === 0, is_today: index === 4 })),
    runs,
    generated_at: '2026-09-28T04:00:00Z',
    version,
  };
}

describe('who may answer and move', () => {
  it('takes answers on the caller own runs that still take them', () => {
    expect(answerable(run('a', 5, '20:00', { mine: true }), ME)).toBe(true);
    expect(answerable(run('a', 5, '20:00', { mine: true, canEdit: false }), ME)).toBe(false);
    expect(answerable(run('a', 5, '20:00'), ME)).toBe(false);
  });

  it('moves only the caller own runs of this boss week that have not started or closed', () => {
    const w = week([]);
    expect(movable(run('a', 5, '20:00', { mine: true }), w, 'this', ME)).toBe(true);
    expect(movable(run('a', 5, '20:00', { mine: true }), w, 'next', ME)).toBe(false);
    expect(movable(run('a', 5, '20:00'), w, 'this', ME)).toBe(false);
    expect(movable(run('a', 5, '20:00', { mine: true, status: 'done' }), w, 'this', ME)).toBe(false);
    expect(movable(run('a', 5, '20:00', { mine: true, status: 'cancelled' }), w, 'this', ME)).toBe(false);
    // Today at 12:00: an 11:00 run has started, a 13:00 one has not.
    expect(movable(run('a', 4, '11:00', { mine: true }), w, 'this', ME)).toBe(false);
    expect(movable(run('a', 4, '13:00', { mine: true }), w, 'this', ME)).toBe(true);
    // Own time: until its day is over.
    expect(movable(run('a', 4, null, { mine: true, status: 'otot' }), w, 'this', ME)).toBe(true);
    expect(movable(run('a', 3, null, { mine: true, status: 'otot' }), w, 'this', ME)).toBe(false);
  });
});

describe('the optimistic answer', () => {
  it('changes only the caller answer, recounts who is on and puts a live run at risk on Out', () => {
    const before = run('a', 5, '20:00', { mine: true, answer: 'yes', status: 'confirmed' });
    const out = withAnswer(before, ME, 'no');
    expect(out.participants.find((p) => p.id === ME)?.answer).toBe('no');
    expect(out.participants.find((p) => p.id === '7')?.answer).toBe('yes');
    expect(out.tally.on).toBe(1);
    expect(out.status).toBe('at_risk');
    expect(before.participants.find((p) => p.id === ME)?.answer).toBe('yes');
    expect(withAnswer(before, ME, 'maybe').status).toBe('confirmed');
    expect(withAnswer(run('a', 5, '20:00', { mine: true, status: 'otot' }), ME, 'no').status).toBe('otot');
  });

  it('shows at once, keeps the server answer when it lands, and rolls back on a stale week', async () => {
    const own = run('a', 5, '20:00', { mine: true, answer: 'waiting' });
    // Each answer the server holds until the test settles it.
    let pending = Promise.withResolvers<unknown>();
    const client = {
      get: vi.fn(async (path: string) => (path.endsWith('next') ? week([], 3) : week([own], 3))),
      put: vi.fn(() => {
        pending = Promise.withResolvers<unknown>();
        return pending.promise;
      }),
    } as unknown as Client;
    const weeks = new MemberWeeks(client, () => false);
    const writes = new RunWrites(client, weeks, () => false);
    await weeks.refresh();

    const saved = writes.answer('a', ME, 'yes');
    expect(weeks.find('a')?.run.participants.find((p) => p.id === ME)?.answer).toBe('yes');
    expect(writes.busy).toBe('a');
    expect(client.put).toHaveBeenCalledWith('/api/public/runs/a/answer', { answer: 'yes', version: 3 });
    pending.resolve({ run: withAnswer(own, ME, 'yes'), version: 4 });
    expect((await saved)?.kind).toBe('done');
    expect(weeks.this?.version).toBe(4);
    expect(writes.busy).toBe('');

    const refused = writes.answer('a', ME, 'no');
    expect(weeks.find('a')?.run.participants.find((p) => p.id === ME)?.answer).toBe('no');
    pending.reject(new ApiRequestError('http', 'Changed meanwhile', 409, { error: 'stale', message: 'Changed meanwhile' }));
    const outcome = await refused;
    expect(outcome?.kind).toBe('stale');
    // The re-read week is the server's: the caller's answer is back to what it was.
    expect(weeks.find('a')?.run.participants.find((p) => p.id === ME)?.answer).toBe('waiting');
    weeks.clear();
  });
});

describe('what changed on a stale write', () => {
  it('names the move, the answer and the place', () => {
    const w = week([]);
    const before = run('a', 5, '20:00', { mine: true });
    expect(changeWords(before, { ...before, day: 6, time: '21:00' }, w, ME)).toBe('A changed meanwhile: it moved to Wed 30 21:00.');
    expect(changeWords(before, run('a', 5, '20:00'), w, ME)).toBe("You're no longer in A.");
    expect(changeWords(before, null, w, ME)).toBe('A is no longer on the week.');
  });
});

describe('move_to and slots', () => {
  it('reads a Discord link instant as a slot of this week and writes it back', () => {
    const w = week([]);
    expect(slotOf('2026-09-29T13:30:00Z', w)).toEqual({ day: 5, time: '21:30' });
    expect(instantOf({ day: 5, time: '21:30' }, w)).toBe('2026-09-29T13:30:00Z');
    for (const slot of [
      { day: 0, time: '00:00' },
      { day: 6, time: '23:45' },
      { day: 4, time: '12:15' },
    ])
      expect(slotOf(instantOf(slot, w)!, w)).toEqual(slot);
  });

  it('has no slot outside the week, for unreadable links or for own time', () => {
    const w = week([]);
    expect(slotOf('2026-10-01T13:30:00Z', w)).toBeNull();
    expect(slotOf('2026-09-23T13:30:00Z', w)).toBeNull();
    expect(slotOf('soon', w)).toBeNull();
    expect(instantOf({ day: 5, time: null }, w)).toBeNull();
  });

  it('says the link ask and the week end on the guild clock', () => {
    expect(askedWords('2026-09-29T13:30:00Z', 'Asia/Singapore')).toBe('Tuesday 21:30');
    expect(askedWords('nope', 'Asia/Singapore')).toBe('');
    expect(weekEnds('2026-09-30T16:00:00Z', 'Asia/Singapore')).toBe('Wed 30 Sep 23:59');
  });
});

describe('the Move page checks', () => {
  const subject = run('a', 5, '20:00', { mine: true });
  const other = run('b', 5, '22:00', { mine: true, others: [] });
  const teammates = run('c', 6, '20:00', { others: [{ id: '7', name: 'Ren', answer: 'yes' }] });
  const w = week([subject, other, teammates]);

  it('passes a free slot inside the week and says who has not answered', () => {
    expect(moveChecks(subject, { day: 6, time: '12:00' }, w, ME)).toEqual({ inside: true, yours: [], mates: [], quiet: ['Mio'] });
  });

  it('blocks a clash with the caller own run and only warns for a teammate', () => {
    expect(moveChecks(subject, { day: 5, time: '21:30' }, w, ME).yours).toEqual(['B 22:00']);
    const mate = moveChecks(subject, { day: 6, time: '20:30' }, w, ME);
    expect(mate.yours).toEqual([]);
    expect(mate.mates).toEqual(['Ren in C 20:00']);
  });

  it('refuses a slot already past or outside the week', () => {
    expect(moveChecks(subject, { day: 4, time: '11:00' }, w, ME).inside).toBe(false);
    expect(moveChecks(subject, { day: 3, time: '20:00' }, w, ME).inside).toBe(false);
    expect(moveChecks(subject, { day: 7, time: '20:00' }, w, ME).inside).toBe(false);
    expect(moveChecks(subject, { day: 4, time: null }, w, ME).inside).toBe(true);
  });
});

describe('what a link shows', () => {
  const link = (over: Partial<MemberRunLink> = {}): MemberRunLink => ({
    run: run('a', 5, '20:00', { mine: true }),
    week: 'current',
    week_starts: '2026-09-24',
    week_ends_at: '2026-09-30T16:00:00Z',
    started: false,
    removed: null,
    this_week: null,
    timing: null,
    generated_at: '2026-09-28T04:00:00Z',
    ...over,
  });

  it('moves a current, unstarted own run and says why anything else is stale', () => {
    expect(linkState(link(), ME)).toBe('move');
    expect(linkState(link({ week: 'past' }), ME)).toBe('week_over');
    expect(linkState(link({ removed: { by: 'Ren', at: '2026-09-27T10:00:00Z' }, run: run('a', 5, '20:00') }), ME)).toBe('not_in');
    expect(linkState(link({ run: run('a', 5, '20:00') }), ME)).toBe('not_in');
    expect(linkState(link({ run: run('a', 5, '20:00', { mine: true, status: 'done' }) }), ME)).toBe('closed');
    expect(linkState(link({ week: 'next' }), ME)).toBe('not_yet');
    expect(linkState(link({ started: true }), ME)).toBe('started');
  });
});

describe('addresses', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('comes back from a fresh sign-in to the run with the choice, or the Move page with the slot', () => {
    expect(answerReturn({ id: 'a' }, 'this', 'no')).toBe('/?run=a&answer=no');
    expect(answerReturn({ id: 'a' }, 'next', 'maybe')).toBe('/?week=next&run=a&answer=maybe');
    expect(moveReturn({ id: 'a b' }, '2026-09-29T13:30:00Z')).toBe('/runs/a%20b?move_to=2026-09-29T13%3A30%3A00Z');
    expect(moveReturn({ id: 'a' }, null)).toBe('/runs/a');
    expect(returnedChoice('maybe')).toBe('maybe');
    expect(returnedChoice('waiting')).toBeNull();
    expect(returnedChoice(null)).toBeNull();
  });

  it('asks the admins about a run or a weekly timing as the request form reads it', () => {
    expect(askPath({ id: 'r1' })).toBe('/requests/new?run=r1');
    expect(weeklyAskPath('f1')).toBe('/requests/new?fixed=f1&change=edit');
  });
});
