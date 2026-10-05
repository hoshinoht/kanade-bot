import { describe, expect, it } from 'vitest';
import { dayOf, daysUntil, minutesUntil, serverNow, span } from '../src/reminders/when';

// Tue 29 Sep 2026, 12:00 in Kuala Lumpur (UTC+8), as the e2e mock is pinned.
const NOW = Date.parse('2026-09-29T04:00:00Z');
const ZONE = 'Asia/Kuala_Lumpur';
// A fire time given as the guild's wall clock (KL is UTC+8 all year).
const kl = (local: string) => new Date(Date.parse(`${local}+08:00`)).toISOString();

describe('reminder times on the server clock', () => {
  it('reads the day part', () => {
    expect(dayOf('Tue 29 Sep 21:00')).toBe('Tue 29 Sep');
  });

  it("advances the server's now by monotonic time only", () => {
    expect(serverNow('2026-09-29T04:00:00Z', 1_000, 1_000)).toBe(NOW);
    expect(serverNow('2026-09-29T04:00:00Z', 1_000, 1_000 + 90 * 60_000)).toBe(NOW + 90 * 60_000);
    // A monotonic reading before the response (never in practice) does not run the clock back.
    expect(serverNow('2026-09-29T04:00:00Z', 5_000, 1_000)).toBe(NOW);
    expect(serverNow('not a time', 0, 0)).toBeNull();
  });

  it('a skewed browser clock does not change "In"', () => {
    const skewed = Date.parse('2026-10-03T23:59:00Z');
    const now = serverNow('2026-09-29T04:00:00Z', 10, 10)!;
    expect(skewed).not.toBe(now);
    expect(span(kl('2026-09-29T21:00'), now, ZONE)).toBe('9 h');
  });

  it('counts minutes to the exact fire time', () => {
    expect(minutesUntil(kl('2026-09-29T21:00'), NOW)).toBe(540);
    expect(minutesUntil(kl('2026-09-29T11:15'), NOW)).toBe(-45);
    expect(minutesUntil('not a time', NOW)).toBeNull();
  });

  it('whole hours under a day, calendar days beyond (B_Reminders "In")', () => {
    expect(span(kl('2026-09-29T12:45'), NOW, ZONE)).toBe('45 min');
    expect(span(kl('2026-09-29T21:45'), NOW, ZONE)).toBe('9 h');
    expect(span(kl('2026-09-29T22:30'), NOW, ZONE)).toBe('10 h');
    expect(span(kl('2026-10-01T09:00'), NOW, ZONE)).toBe('2 d');
    expect(span(kl('2026-10-05T20:45'), NOW, ZONE)).toBe('6 d');
    expect(span(kl('2026-09-27T20:00'), NOW, ZONE)).toBe('2 d');
  });

  it('knows today on the guild wall clock', () => {
    expect(daysUntil(kl('2026-09-29T23:59'), NOW, ZONE)).toBe(0);
    expect(daysUntil(kl('2026-09-30T00:00'), NOW, ZONE)).toBe(1);
    expect(daysUntil(kl('2027-01-01T09:00'), Date.parse('2026-12-31T12:00:00Z'), ZONE)).toBe(1);
  });
});

import { cardLines, lead } from '../src/reminders/discord';

describe('card text', () => {
  it('splits bold runs and lines, keeping spaces', () => {
    expect(cardLines('⏰ Onward! · **HCarling** in 1h\nnext')).toEqual([
      [
        { bold: false, text: '⏰ Onward! · ' },
        { bold: true, text: 'HCarling' },
        { bold: false, text: ' in 1h' },
      ],
      [{ bold: false, text: 'next' }],
    ]);
    expect(cardLines('a **b')).toEqual([[{ bold: false, text: 'a **b' }]]);
    expect(lead(' · x')).toEqual([' ', '· x']);
  });
});
