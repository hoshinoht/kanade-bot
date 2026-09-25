import { describe, expect, it } from 'vitest';
import { activeCount, NO_LOG_FILTER, parseFilter, preset, toSearch } from '../src/logs/filters';

const week = {
  days: ['2026-09-24', '2026-09-25', '2026-09-26', '2026-09-27', '2026-09-28', '2026-09-29', '2026-09-30'].map((date) => ({
    date,
    is_today: date === '2026-09-29',
  })),
};

describe('log filters', () => {
  it('round-trips through the query string, outcomes as one list', () => {
    const search = '?model=kanata%2Fchat&from=2026-09-24&outcome=timeout,error&min_ms=5000';
    const filter = parseFilter(search);
    expect(filter.outcome).toEqual(['timeout', 'error']);
    expect(filter.model).toBe('kanata/chat');
    expect(parseFilter(toSearch(filter))).toEqual(filter);
    expect(activeCount(filter)).toBe(4);
    expect(toSearch(NO_LOG_FILTER)).toBe('');
  });

  it('drops Chat-only keys for Extractions instead of counting them', () => {
    const filter = parseFilter('?outcome=proposed&tool=schedule.read&min_ms=5000', { chat: false });
    expect(filter.tool).toBe('');
    expect(filter.min_ms).toBe('');
    expect(activeCount(filter)).toBe(1);
    expect(toSearch(filter)).toBe('?outcome=proposed');
  });

  it('computes guild-timezone presets from the week the server sent', () => {
    expect(preset('today', week)).toEqual({ from: '2026-09-29', to: '2026-09-29' });
    expect(preset('week', week)).toEqual({ from: '2026-09-24', to: '2026-09-30' });
    expect(preset('7d', week)).toEqual({ from: '2026-09-23', to: '2026-09-29' });
  });
});
