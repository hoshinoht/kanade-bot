import { describe, expect, it } from 'vitest';
import { validClock } from '../src/config/headerTime';

describe('header generation time', () => {
  it('takes HH:MM in 24-hour time only', () => {
    for (const ok of ['00:00', '03:30', '23:59', ' 09:05 ']) expect(validClock(ok)).toBe(true);
    for (const bad of ['', '3:30', '24:00', '03:60', '0330', 'noon']) expect(validClock(bad)).toBe(false);
  });
});
