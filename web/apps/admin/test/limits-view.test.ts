import { describe, expect, it, vi } from 'vitest';
import { atCapacity, GUILD_ZONE, phoneOrder, refusalGroups, resetSpan, serverTime, windowWords } from '../src/limits/view';

describe('limits view', () => {
  it('prints a server instant on the guild clock with fixed English names', () => {
    expect(serverTime('2026-09-29T04:00:00Z', 'Asia/Singapore')).toBe('Tue 29 Sep 12:00');
    expect(serverTime('2026-09-29T23:05:00Z', 'Asia/Singapore')).toBe('Wed 30 Sep 07:05');
    // An older server's ready-made label prints as sent.
    expect(serverTime('Tue 29 Sep 11:56', 'Asia/Singapore')).toBe('Tue 29 Sep 11:56');
  });

  it('falls back to the guild zone, never the browser zone, before the week names one', () => {
    // A browser far from the guild: the old fallback printed New York's clock.
    const spy = vi.spyOn(Intl.DateTimeFormat.prototype, 'resolvedOptions').mockReturnValue({
      ...new Intl.DateTimeFormat().resolvedOptions(),
      timeZone: 'America/New_York',
    });
    try {
      expect(GUILD_ZONE).toBe('Asia/Kuala_Lumpur');
      expect(serverTime('2026-09-29T16:30:00Z', '')).toBe('Wed 30 Sep 00:30');
      expect(serverTime('2026-09-29T16:30:00Z', undefined)).toBe('Wed 30 Sep 00:30');
    } finally {
      spy.mockRestore();
    }
  });

  it('orders open, then half-open breakers first on phones, otherwise as configured', () => {
    const g = (name: string, state: 'closed' | 'half_open' | 'open') => ({ name, breaker: { state, failures: 0, since: '' } });
    const order = phoneOrder([g('cloud', 'closed'), g('local', 'half_open'), g('legacy', 'open'), g('spare', 'closed')]);
    expect(order.map((x) => x.name)).toEqual(['legacy', 'local', 'cloud', 'spare']);
  });

  it('names the first group with every permit held', () => {
    const g = (name: string, in_use: number, total: number) => ({ name, permits: { in_use, total } });
    expect(atCapacity([g('a', 1, 2), g('b', 2, 2)])?.name).toBe('b');
    expect(atCapacity([g('a', 0, 0)])).toBeUndefined();
  });

  it('splits refusals into backend groups then the gateway key, with totals', () => {
    const r = (kind: string, scope: string, count: number) => ({ kind, scope, target: 't', count, last_at: '' });
    const groups = refusalGroups([r('key_rate', 'key', 2), r('rate', 'group', 3), r('quota', 'key', 1)]);
    expect(groups.map((x) => [x.label, x.total, x.rows.length])).toEqual([
      ['Backend groups', 3, 1],
      ['Gateway key', 3, 2],
    ]);
    expect(refusalGroups([])).toEqual([]);
  });
});

describe('allowance reset countdown', () => {
  const now = Date.parse('2026-09-29T04:00:00Z');
  it('reads like the boards: days and hours, hours and minutes, minutes and seconds', () => {
    expect(resetSpan('2026-09-29T09:12:00Z', now)).toBe('5 h 12 m');
    expect(resetSpan('2026-09-30T07:00:00Z', now)).toBe('1 d 3 h');
    expect(resetSpan('2026-09-29T04:02:12Z', now)).toBe('2 m 12 s');
    expect(resetSpan('2026-09-29T04:00:45Z', now)).toBe('45 s');
    // Hours round up to the minute: one second gone still reads 5 h 12 m.
    expect(resetSpan('2026-09-29T09:12:00Z', now + 1000)).toBe('5 h 12 m');
    expect(resetSpan('2026-09-29T09:11:00Z', now)).toBe('5 h 11 m');
    expect(resetSpan('2026-09-29T05:00:00Z', now)).toBe('1 h 0 m');
    expect(resetSpan('2026-09-29T04:59:59Z', now)).toBe('59 m 59 s');
    expect(resetSpan('2026-09-30T03:59:59Z', now)).toBe('1 d 0 h');
    // A part second left still reads as a second, never as due.
    expect(resetSpan('2026-09-29T04:00:01Z', now + 200)).toBe('1 s');
  });

  it('is due at the instant, and absent without a reset or a server clock', () => {
    expect(resetSpan('2026-09-29T04:00:00Z', now)).toBe('');
    expect(resetSpan('2026-09-29T03:59:00Z', now)).toBe('');
    expect(resetSpan(null, now)).toBeNull();
    expect(resetSpan('soon', now)).toBeNull();
    expect(resetSpan('2026-09-29T04:01:00Z', null)).toBeNull();
  });
});

describe('allowance windows in words', () => {
  it('reads seconds as the largest whole unit, shared with Account', () => {
    expect(windowWords(21_600)).toBe('6 h');
    expect(windowWords(300)).toBe('5 min');
    expect(windowWords(172_800)).toBe('2 d');
    expect(windowWords(90)).toBe('90 s');
  });
});
