import { describe, expect, it } from 'vitest';
import { activeCount, NO_LOG_FILTER, parseFilter, toSearch } from '../src/logs/filters';

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
});
