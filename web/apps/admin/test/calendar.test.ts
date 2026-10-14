import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  dayLabel,
  dayOf,
  isoOf,
  monthCells,
  ordered,
  rangeChips,
  rangeWords,
  serverClock,
  sinceChips,
  typedError,
  weekHeads,
  weekStartOf,
  zoneWords,
} from '../../../packages/ui/src/components/calendar';

const THU = 4;
const day = (iso: string) => dayOf(iso)!;
const week = (generated_at: string, reset = 'Thu 00:00') => ({ generated_at, timezone: 'Asia/Kuala_Lumpur', reset });

describe('date picker month grid', () => {
  it('starts every row on the reset day, so a boss week is one row', () => {
    expect(weekHeads(THU).map((h) => h.short)).toEqual(['THU', 'FRI', 'SAT', 'SUN', 'MON', 'TUE', 'WED']);
    expect(weekHeads(THU)[0]!.long).toBe('Thursday (reset)');
    const cells = monthCells(2026, 8, THU);
    expect(cells).toHaveLength(42);
    expect(isoOf(cells[0]!)).toBe('2026-08-27');
    const row = cells.slice(28, 35).map(isoOf);
    expect(row).toEqual(['2026-09-24', '2026-09-25', '2026-09-26', '2026-09-27', '2026-09-28', '2026-09-29', '2026-09-30']);
    for (let r = 0; r < 6; r++) expect(dayLabel(cells[r * 7]!).startsWith('Thu')).toBe(true);
  });

  it('leads with the previous boss week when the month opens on reset day', () => {
    // 1 Oct 2026 is a Thursday.
    expect(isoOf(monthCells(2026, 9, THU)[0]!)).toBe('2026-09-24');
    expect(isoOf(weekStartOf(day('2026-10-04'), THU))).toBe('2026-10-01');
  });
});

describe('server clock', () => {
  afterEach(() => vi.useRealTimers());

  it('takes today and the boss week from Week.generated_at in the guild zone, never the browser clock', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2031-01-01T00:00:00Z'));
    const clock = serverClock(week('2026-09-29T04:00:00Z'))!;
    expect(isoOf(clock.today)).toBe('2026-09-29');
    expect(isoOf(clock.weekStart)).toBe('2026-09-24');
    expect(clock.firstDow).toBe(THU);
    // 17:00 UTC on Wed 30 Sep is 01:00 Thu 1 Oct in Kuala Lumpur: a new boss week.
    const late = serverClock(week('2026-09-30T17:00:00Z'))!;
    expect(isoOf(late.today)).toBe('2026-10-01');
    expect(isoOf(late.weekStart)).toBe('2026-10-01');
  });

  it('keeps last boss week until the reset time on reset day', () => {
    const early = serverClock(week('2026-09-30T17:00:00Z', 'Thu 08:00'))!;
    expect(isoOf(early.weekStart)).toBe('2026-09-24');
    expect(serverClock(week('2026-09-29T04:00:00Z', 'Mon 00:00'))!.firstDow).toBe(1);
    expect(serverClock(null)).toBeNull();
    expect(serverClock(week('not a time'))).toBeNull();
  });

  it('names the zone in words', () => {
    expect(zoneWords('Asia/Kuala_Lumpur')).toBe('Kuala Lumpur time');
    expect(zoneWords('UTC')).toBe('UTC');
  });
});

describe('quick picks', () => {
  const clock = serverClock(week('2026-09-29T04:00:00Z'))!;
  const words = (from: number, to: number) => [isoOf(from), isoOf(to)];

  it('range: this boss week to today, last boss week whole, last 7 days', () => {
    expect(rangeChips(clock).map((c) => [c.label, ...words(c.from, c.to)])).toEqual([
      ['This boss week', '2026-09-24', '2026-09-29'],
      ['Last boss week', '2026-09-17', '2026-09-23'],
      ['Last 7 days', '2026-09-23', '2026-09-29'],
    ]);
  });

  it('since: the last reset, a week ago, and the start of last boss week', () => {
    expect(sinceChips(clock).map((c) => [c.label, isoOf(c.day)])).toEqual([
      ['Last reset · Thu 24', '2026-09-24'],
      ['7 days ago', '2026-09-22'],
      ['Last boss week', '2026-09-17'],
    ]);
  });
});

describe('range wording and order', () => {
  it('puts the earlier day first, whichever was picked first', () => {
    expect(ordered(day('2026-10-02'), day('2026-09-28')).map(isoOf)).toEqual(['2026-09-28', '2026-10-02']);
    expect(ordered(day('2026-09-28'), day('2026-10-02')).map(isoOf)).toEqual(['2026-09-28', '2026-10-02']);
  });

  it('spells the range out, and a one-day range is one date', () => {
    expect(rangeWords(day('2026-09-24'), day('2026-09-30'))).toBe('Thu 24 Sep – Wed 30 Sep');
    expect(rangeWords(day('2026-10-04'), day('2026-10-04'))).toBe('Sun 04 Oct');
    expect(rangeWords(day('2026-09-24'), null)).toBe('from Thu 24 Sep');
    expect(rangeWords(null, null)).toBe('');
    expect(dayOf('')).toBeNull();
    expect(dayOf('2026-02-30')).toBeNull();
  });
});

describe('typed From/To fields', () => {
  const today = day('2026-10-04');

  it('says which field is wrong, in the boards’ words', () => {
    expect(typedError('2026-10-02', '2026-09-28', today)).toEqual({ message: 'To (Mon 28 Sep) is before From (Fri 02 Oct).', field: 'to' });
    expect(typedError('2026-02-30', '2026-10-04', today)).toEqual({ message: 'That day doesn’t exist in that month.', field: 'from' });
    expect(typedError('2026-09-28', '28/09', today)).toEqual({ message: 'Use a date like 2026-09-28.', field: 'to' });
    expect(typedError('2026-09-28', '2026-10-05', today)).toEqual({ message: 'To is after today (Sun 04 Oct).', field: 'to' });
  });

  it('accepts a good pair, single-digit months and days included', () => {
    expect(typedError('2026-9-28', '2026-10-04', today)).toBeNull();
    expect(typedError('2026-10-04', '2026-10-04', today)).toBeNull();
  });
});
