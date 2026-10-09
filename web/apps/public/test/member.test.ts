import { describe, expect, it } from 'vitest';
import type { Answer, MemberRun, MemberWeek, RunStatus } from '@kanade/api-types';
import { answersOwed, countdownWords, nextOwn, partyLine, shownRuns, youFirst, yours } from '../src/member';

const ME = '42';

function run(id: string, day: number, time: string | null, options: { mine?: boolean; answer?: Answer; status?: RunStatus; canEdit?: boolean } = {}): MemberRun {
  const mine = options.mine ?? false;
  const participants = [
    { id: '7', name: 'Ren', answer: 'yes' as Answer },
    ...(mine ? [{ id: ME, name: 'Asahi', answer: options.answer ?? ('yes' as Answer) }] : []),
    { id: '9', name: 'Mio', answer: 'maybe' as Answer },
  ];
  return {
    id,
    day,
    time,
    minutes: 60,
    status: options.status ?? 'planned',
    bosses: [],
    tally: { on: 2, total: participants.length },
    participants,
    party: '',
    channel: '',
    fixed_id: null,
    mine,
    can_edit: options.canEdit ?? true,
  };
}

/** Wed 23 Sep 2026 (Thu reset) … Tue 29 Sep; the server's clock is Mon 28 Sep 12:00 guild time. */
function week(runs: MemberRun[]): MemberWeek {
  const dows = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'];
  return {
    starts: '2026-09-24',
    timezone: 'Asia/Singapore',
    reset: 'Thu 08:00',
    days: dows.map((dow, index) => ({ index, date: `2026-09-${String(24 + index).padStart(2, '0')}`, dow, is_reset: index === 0, is_today: index === 4 })),
    runs,
    generated_at: '2026-09-28T04:00:00Z', // 12:00 in Singapore
    version: 1,
  };
}

describe('own and other runs', () => {
  it('finds the caller on their own runs only', () => {
    expect(yours(run('a', 0, '20:00', { mine: true, answer: 'waiting' }), ME)).toEqual({ id: ME, name: 'Asahi', answer: 'waiting' });
    expect(yours(run('b', 0, '20:00'), ME)).toBeNull();
  });

  it('puts "You" first on the party line and the party list of an own run', () => {
    const own = run('a', 0, '20:00', { mine: true });
    expect(partyLine(own, ME)).toBe('You · Ren · Mio');
    expect(youFirst(own, ME).map((p) => p.id)).toEqual([ME, '7', '9']);
    expect(partyLine(run('b', 0, '20:00'), ME)).toBe('Ren · Mio');
  });

  it('filters to the caller and hides the past until asked', () => {
    const runs = [run('own', 1, '20:00', { mine: true }), run('other', 1, '21:00'), run('done', 0, '20:00', { mine: true, status: 'done' }), run('gone', 0, '21:00', { status: 'cancelled' })];
    expect(shownRuns(runs, { onlyMine: false, showPast: false }).map((r) => r.id)).toEqual(['own', 'other']);
    expect(shownRuns(runs, { onlyMine: true, showPast: false }).map((r) => r.id)).toEqual(['own']);
    expect(shownRuns(runs, { onlyMine: true, showPast: true }).map((r) => r.id)).toEqual(['own', 'done']);
    expect(shownRuns(runs, { onlyMine: false, showPast: true })).toHaveLength(4);
  });
});

describe('Needs you', () => {
  it('lists own runs still taking answers with none from the caller, in board order', () => {
    const runs = [
      run('later', 5, '20:00', { mine: true, answer: 'waiting' }),
      run('sooner', 4, '21:00', { mine: true, answer: 'waiting' }),
      run('answered', 4, '19:00', { mine: true, answer: 'yes' }),
      run('locked', 4, '22:00', { mine: true, answer: 'waiting', canEdit: false }),
      run('theirs', 4, '23:00'),
    ];
    expect(answersOwed(runs, ME).map((r) => r.id)).toEqual(['sooner', 'later']);
  });
});

describe('next own run', () => {
  it('is the soonest own timed run still ahead of the server clock', () => {
    const w = week([
      run('started', 4, '11:00', { mine: true }),
      run('other', 4, '13:00'),
      run('tomorrow', 5, '12:00', { mine: true }),
      run('tonight', 4, '20:00', { mine: true }),
      run('done', 4, '18:00', { mine: true, status: 'done' }),
    ]);
    expect(nextOwn(w)?.id).toBe('tonight');
    expect(countdownWords(nextOwn(w)!, w)).toBe('in 8 h');
    expect(countdownWords(w.runs[2]!, w)).toBe('in 1 day');
    expect(countdownWords(w.runs[0]!, w)).toBe('');
  });

  it('falls back to an own-time run from today on, else nothing', () => {
    const ownTime = week([run('past', 3, null, { mine: true }), run('free', 6, null, { mine: true }), run('started', 4, '11:00', { mine: true })]);
    expect(nextOwn(ownTime)?.id).toBe('free');
    expect(countdownWords(ownTime.runs[1]!, ownTime)).toBe('');
    expect(nextOwn(week([run('other', 5, '20:00'), run('started', 4, '11:00', { mine: true })]))).toBeNull();
  });
});
