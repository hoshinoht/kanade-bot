import { describe, expect, it } from 'vitest';
import { dayOf, daysUntil, minutesUntil, span } from '../src/reminders/when';

// Tue 29 Sep 2026, 12:00 in Kuala Lumpur (UTC+8), as the e2e mock is pinned.
const NOW = new Date('2026-09-29T04:00:00Z');
const ZONE = 'Asia/Kuala_Lumpur';

describe('reminder times', () => {
  it('reads the day part', () => {
    expect(dayOf('Tue 29 Sep 21:00')).toBe('Tue 29 Sep');
  });

  it('counts wall-clock minutes in the guild zone', () => {
    expect(minutesUntil('Tue 29 Sep 21:00', NOW, ZONE)).toBe(540);
    expect(minutesUntil('Tue 29 Sep 11:15', NOW, ZONE)).toBe(-45);
    expect(minutesUntil('not a time', NOW, ZONE)).toBeNull();
  });

  it('whole hours under a day, calendar days beyond (B_Reminders "In")', () => {
    expect(span('Tue 29 Sep 12:45', NOW, ZONE)).toBe('45 min');
    expect(span('Tue 29 Sep 21:45', NOW, ZONE)).toBe('9 h');
    expect(span('Tue 29 Sep 22:30', NOW, ZONE)).toBe('10 h');
    expect(span('Thu 01 Oct 09:00', NOW, ZONE)).toBe('2 d');
    expect(span('Mon 05 Oct 20:45', NOW, ZONE)).toBe('6 d');
    expect(span('Sun 27 Sep 20:00', NOW, ZONE)).toBe('2 d');
  });

  it('knows today and takes the year nearest to now', () => {
    expect(daysUntil('Tue 29 Sep 23:59', NOW, ZONE)).toBe(0);
    expect(daysUntil('Wed 30 Sep 00:00', NOW, ZONE)).toBe(1);
    const newYear = new Date('2026-12-31T12:00:00Z');
    expect(daysUntil('Fri 01 Jan 09:00', newYear, ZONE)).toBe(1);
  });
});
