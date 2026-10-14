import { describe, expect, it } from 'vitest';
import { duration, logTime } from '../src/logs/format';

describe('log formats', () => {
  it('shows server instants in guild time, the full stamp in the tooltip', () => {
    expect(logTime('2026-09-23T16:34:41Z', 'Asia/Kuala_Lumpur')).toEqual({
      text: 'Thu 24 Sep · 00:34',
      title: 'Thu 24 Sep 2026, 00:34:41 (Asia/Kuala_Lumpur)',
      iso: '2026-09-23T16:34:41Z',
    });
    // Text the server already formatted passes through.
    expect(logTime('Tue 29 Sep 12:00', 'Asia/Kuala_Lumpur')).toEqual({ text: 'Tue 29 Sep 12:00', title: 'Tue 29 Sep 12:00', iso: null });
  });

  it('keeps durations compact', () => {
    expect(duration(545)).toBe('545 ms');
    expect(duration(4_910)).toBe('4.9 s');
    expect(duration(12_300)).toBe('12 s');
    expect(duration(60_000)).toBe('1.0 min');
    expect(duration(0)).toBe('—');
    expect(duration(null)).toBe('—');
  });
});
