import { describe, expect, it } from 'vitest';
import type { Proposal, Run, Week, WeekDay } from '@kanade/api-types';
import { editText, editWeek, isoToday, parseEdit } from '../src/inbox/edit';

// The boards' boss week: Thu 01 – Wed 07, today Sun 04, reset Thu.
const days: WeekDay[] = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'].map((dow, index) => ({
  index,
  dow,
  date: `2026-10-0${index + 1}`,
  is_reset: index === 0,
  is_today: index === 3,
}));

describe('Inbox edit week', () => {
  const week = { days, runs: [{ id: 'r-limbo', day: 5, time: '23:30', status: 'planned' } as Run], reset: 'Thu 00:00' } as Week;
  const proposal = (when: string, from: string | null = null) => ({ when, from_when: from, run_id: 'r-limbo' }) as Proposal;

  it('uses the week on screen when the proposal falls in it', () => {
    const at = editWeek(proposal('Wed 07 Oct 23:30'), week, 'Thu')!;
    expect(at.days).toBe(days);
    expect(at.slot).toEqual({ day: 6, time: '23:30' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
  });

  it('falls back to weekday names for another week', () => {
    const at = editWeek(proposal('Wed 14 Oct 21:00', 'Tue 13 Oct 23:30'), week, 'Thu')!;
    expect(at.days.map((d) => d.dow)).toEqual(['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed']);
    expect(at.days.every((d) => !d.date && !d.is_today)).toBe(true);
    expect(at.runs).toEqual([]);
    expect(at.slot).toEqual({ day: 6, time: '21:00' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
  });

  it('dates its own boss week from today when it is not on screen', () => {
    const today = isoToday('2026-09-29T04:00:00Z', 'Asia/Kuala_Lumpur');
    expect(today).toBe('2026-09-29');
    const at = editWeek(proposal('Wed 30 Sep 23:30', 'Tue 29 Sep 23:30'), null, 'Thu', today)!;
    expect(at.days.map((d) => d.date.slice(8))).toEqual(['24', '25', '26', '27', '28', '29', '30']);
    expect(at.days.findIndex((d) => d.is_today)).toBe(5);
    expect(at.slot).toEqual({ day: 6, time: '23:30' });
    expect(at.own).toEqual({ day: 5, time: '23:30' });
    // Across the new year, the year nearest today.
    const jan = editWeek(proposal('Fri 01 Jan 21:00'), null, 'Thu', '2026-12-30')!;
    expect(jan.days[0]!.date).toBe('2026-12-31');
  });

  it('round-trips the picked slot through parseEdit', () => {
    const p = { ...proposal('Wed 07 Oct 23:30'), when: 'Wed 07 Oct 23:30' } as Proposal;
    expect(parseEdit(editText({ day: 4, time: '22:30' }, days), p, 'Thu')).toEqual({ ok: true, day: 4, time: '22:30' });
  });
});
