import { describe, expect, it } from 'vitest';
import type { WeekDay } from '@kanade/api-types';
import { parseWhen } from '../src/sheet/parseWhen';

const days: WeekDay[] = ['Thu', 'Fri', 'Sat', 'Sun', 'Mon', 'Tue', 'Wed'].map((dow, index) => ({
  index,
  dow,
  date: `2026-09-${24 + index}`,
  is_reset: index === 0,
  is_today: false,
}));
const current = { day: 5, time: '22:00' };

describe('parseWhen', () => {
  it.each([
    ['wed 21:30', { day: 6, time: '21:30' }],
    ['Wednesday 9:45pm', { day: 6, time: '21:45' }],
    ['fri 12am', { day: 1, time: '00:00' }],
    ['22:30', { day: 5, time: '22:30' }],
    ['mon', { day: 4, time: '22:00' }],
    ['  sat 7pm ', { day: 2, time: '19:00' }],
  ])('reads %s', (text, slot) => {
    expect(parseWhen(text, days, current)).toEqual({ ok: true, slot });
  });

  it.each(['', 'soon', '25:00', '9', '13pm', 'wed 21:75'])('rejects %s with a reason', (text) => {
    const result = parseWhen(text, days, current);
    expect(result.ok).toBe(false);
    expect(result.ok ? '' : result.message).not.toBe('');
  });
});
