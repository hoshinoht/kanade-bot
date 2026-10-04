import type { ConfigView, Week } from '@kanade/api-types';
import { describe, expect, it } from 'vitest';
import { groupCap, wavingRows, type GroupRow } from '../src/config/capacity';
import { EXPIRY_WARN, proposalExpiry } from '../src/inbox/expiry';
import { MAX_WAVES, wavingGroups } from '../src/limits/permits';
import { dateMinutes, spanWords, wallMinutes, whenMinutes } from '../src/shared/wall';
import { COUNTDOWN_SPAN, runCountdown, weekProgress } from '../src/week/progress';

const TZ = 'Asia/Kuala_Lumpur';
const DAYS = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'];

/** A boss week from Thu 24 Sep 2026; `today` is the day index the server says is today. */
function week(nowUtc: string, today: number | null, reset = 'Thu 00:00'): Pick<Week, 'days' | 'reset' | 'generated_at' | 'timezone'> {
  return {
    reset,
    timezone: TZ,
    generated_at: nowUtc,
    days: DAYS.map((dow, index) => ({ index, dow, date: `2026-09-${String(24 + index).padStart(2, '0')}`, is_reset: index === 0, is_today: index === today })),
  };
}

describe('wall clock', () => {
  it('reads an instant on the guild clock, and local dates and labels the same way', () => {
    // 04:00Z is 12:00 in Kuala Lumpur (UTC+8).
    expect(wallMinutes('2026-09-29T04:00:00Z', TZ)).toBe(dateMinutes('2026-09-29', '12:00'));
    expect(wallMinutes('not a date', TZ)).toBeNull();
    expect(dateMinutes('2026-09-29', 'noon')).toBeNull();
  });

  it('gives a yearless label the year nearest now', () => {
    const now = dateMinutes('2026-12-31', '20:00')!;
    expect(whenMinutes('Fri 01 Jan 08:00', now)).toBe(dateMinutes('2027-01-01', '08:00'));
    expect(whenMinutes('Tue 29 Dec 08:00', now)).toBe(dateMinutes('2026-12-29', '08:00'));
    expect(whenMinutes('Tue 29 Foo 08:00', now)).toBeNull();
  });

  it('words spans like the API countdown', () => {
    expect([spanWords(47), spanWords(120), spanWords(125), spanWords(24 * 60 + 5), spanWords(3 * 24 * 60)]).toEqual(['47 min', '2 h', '2 h 5 min', '1 day', '3 days']);
  });
});

describe('boss week progress', () => {
  it('says the day and the reset, filled to the minute since the reset', () => {
    // Tue 29 Sep 12:00 local is day 6 of the week that began Thu 24 Sep 00:00.
    const bar = weekProgress(week('2026-09-29T04:00:00Z', 5))!;
    expect(bar.text).toBe('Day 6 of 7 · resets Thu 00:00');
    expect(bar.max).toBe(7 * 1440);
    expect(bar.value).toBe(5 * 1440 + 12 * 60);
  });

  it('starts from a later reset time and stays in range', () => {
    const bar = weekProgress(week('2026-09-24T02:00:00Z', 0, 'Thu 08:00'))!;
    expect(bar.value).toBe(2 * 60);
    expect(weekProgress(week('2026-09-23T20:00:00Z', 0, 'Thu 08:00'))!.value).toBe(0);
  });

  it('is absent for a week that is not running', () => {
    expect(weekProgress(week('2026-09-29T04:00:00Z', null))).toBeNull();
  });
});

describe('run countdown', () => {
  const now = week('2026-09-29T04:00:00Z', 5); // Tue 12:00
  const run = (day: number, time: string | null, status: 'planned' | 'otot' | 'done' | 'cancelled' = 'planned') => ({ day, time, status });

  it('waves over the final 24 h, with marks at T-1h and T-15m', () => {
    const c = runCountdown(run(5, '21:00'), now)!;
    expect(c.left).toBe(9 * 60);
    expect(c.wavy).toBe(true);
    expect(c.value).toBe(COUNTDOWN_SPAN - 9 * 60);
    expect(c.ticks).toEqual([(1440 - 60) / 1440, (1440 - 15) / 1440]);
    expect(c.text).toBe('starts in 9 h');
  });

  it('is flat and empty earlier than a day out, and wavy exactly at 24 h', () => {
    const later = runCountdown(run(6, '13:00'), now)!;
    expect([later.wavy, later.value, later.text]).toEqual([false, 0, 'starts in 1 day']);
    const edge = runCountdown(run(6, '12:00'), now)!;
    expect([edge.wavy, edge.value]).toEqual([true, 0]);
  });

  it('is absent for own-time, finished, cancelled and started runs', () => {
    expect(runCountdown(run(5, null), now)).toBeNull();
    expect(runCountdown(run(5, '21:00', 'otot'), now)).toBeNull();
    expect(runCountdown(run(5, '21:00', 'done'), now)).toBeNull();
    expect(runCountdown(run(5, '21:00', 'cancelled'), now)).toBeNull();
    expect(runCountdown(run(5, '12:00'), now)).toBeNull();
    expect(runCountdown(run(2, '21:00'), now)).toBeNull();
  });
});

describe('proposal expiry', () => {
  const NOW = '2026-09-29T04:00:00Z'; // Tue 29 Sep 12:00 local
  const item = (read_at: string, expires_at: string | null, flags: string[] = []) => ({ read_at, expires_at, flags }) as Parameters<typeof proposalExpiry>[0];

  it('drains from read to expiry against the server clock', () => {
    const e = proposalExpiry(item('Tue 29 Sep 08:00', 'Wed 30 Sep 08:00'), NOW, TZ)!;
    expect([e.span, e.left, e.warn, e.text]).toEqual([24 * 60, 20 * 60, false, '20 h left']);
  });

  it('warns in its last fifth', () => {
    const e = proposalExpiry(item('Mon 28 Sep 16:00', 'Tue 29 Sep 16:00'), NOW, TZ)!;
    expect(e.left).toBe(4 * 60);
    expect(e.left <= e.span * EXPIRY_WARN).toBe(true);
    expect([e.warn, e.text]).toEqual([true, '4 h left, expiring soon']);
    expect(proposalExpiry(item('Mon 28 Sep 17:00', 'Tue 29 Sep 17:00'), NOW, TZ)!.warn).toBe(false);
  });

  it('is absent without an expiry, once expired, past its time, or without a clock', () => {
    expect(proposalExpiry(item('Tue 29 Sep 08:00', null), NOW, TZ)).toBeNull();
    expect(proposalExpiry(item('Tue 29 Sep 08:00', 'Wed 30 Sep 08:00', ['expired']), NOW, TZ)).toBeNull();
    expect(proposalExpiry(item('Mon 28 Sep 08:00', 'Tue 29 Sep 11:00'), NOW, TZ)).toBeNull();
    expect(proposalExpiry(item('Tue 29 Sep 08:00', 'Wed 30 Sep 08:00'), '', TZ)).toBeNull();
  });
});

describe('permit bars', () => {
  const g = (name: string, in_use: number, total: number) => ({ name, permits: { in_use, total } });

  it('wave only with requests in flight, the fullest first, at most two', () => {
    expect(MAX_WAVES).toBe(2);
    expect(wavingGroups([g('a', 1, 4), g('b', 0, 1), g('c', 1, 1), g('d', 3, 4)])).toEqual(new Set(['c', 'd']));
    expect(wavingGroups([g('a', 0, 1), g('b', 0, 0)]).size).toBe(0);
  });

  it('wave on Config while calls hold a group, the fullest first, at most two', () => {
    const row = (group: string, inUse: number | null, permits: number | null): GroupRow => ({ group, permits, inUse, models: [] });
    expect(wavingRows([row('a', 1, 4), row('b', 0, 1), row('c', 1, 1), row('d', 3, 4)])).toEqual(new Set(['c', 'd']));
    // No governor (null) or no permits: flat.
    expect(wavingRows([row('a', null, 1), row('b', 1, null)]).size).toBe(0);
  });

  it('measure Config permits against the least Kanata admits in the group', () => {
    const models = {
      groups: [
        { model: 'x', group: 'main', permits: 2 },
        { model: 'y', group: 'main', permits: 2 },
        { model: 'z', group: 'solo', permits: 1 },
      ],
      alias_limits: [
        { alias: 'x', max_in_flight: 4, source: 'published' },
        { alias: 'y', max_in_flight: 6, adapter_max_in_flight: 3, source: 'declared' },
      ],
    } as unknown as ConfigView['models'];
    expect(groupCap(models, 'main')).toBe(3);
    expect(groupCap(models, 'solo')).toBeNull();
  });
});
