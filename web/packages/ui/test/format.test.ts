import { describe, expect, it } from 'vitest';
import { weekStartLabel } from '../src/format';

describe('weekStartLabel', () => {
  it('names the weekday from the calendar date', () => {
    expect(weekStartLabel('2026-09-24')).toBe('Thu 24 Sep');
    expect(weekStartLabel('2026-10-01')).toBe('Thu 1 Oct');
    expect(weekStartLabel('2027-01-03')).toBe('Sun 3 Jan');
  });
});
