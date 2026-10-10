import { describe, expect, it } from 'vitest';
import { callOf, matches, pretty, rowOutcome, segments, timeRange, withCall } from '../src/extractions/code';

describe('extractions list-detail helpers', () => {
  it('keeps the filters when the selection changes', () => {
    expect(withCall('?outcome=proposed&q=kalos', 'x-kalos')).toBe('?outcome=proposed&q=kalos&call=x-kalos');
    expect(withCall('?outcome=proposed&call=x-bm', 'x-kalos')).toBe('?outcome=proposed&call=x-kalos');
    expect(withCall('?call=x-bm', '')).toBe('');
    expect(callOf('?model=a&call=x-bm')).toBe('x-bm');
    expect(callOf('')).toBe('');
  });

  it('words a row outcome: changes, no change, or the outcome itself', () => {
    expect(rowOutcome({ outcome: 'proposed', changes: 1 })).toEqual({ text: '1 change', tone: 'success' });
    expect(rowOutcome({ outcome: 'proposed', changes: 3 })).toEqual({ text: '3 changes', tone: 'success' });
    expect(rowOutcome({ outcome: 'no_change', changes: 0 })).toEqual({ text: 'no change', tone: 'neutral' });
    expect(rowOutcome({ outcome: 'failed', changes: 0 })).toEqual({ text: 'failed', tone: 'danger' });
    expect(rowOutcome({ outcome: 'self_service_link', changes: 0 }).tone).toBe('info');
  });

  it('splits a line around case-insensitive matches', () => {
    expect(segments('Kalos at 22:00, kalos again', 'KALOS')).toEqual([
      { text: 'Kalos', hit: true },
      { text: ' at 22:00, ', hit: false },
      { text: 'kalos', hit: true },
      { text: ' again', hit: false },
    ]);
    expect(segments('', 'x')).toEqual([{ text: '', hit: false }]);
    expect(segments('abc', '')).toEqual([{ text: 'abc', hit: false }]);
    expect(matches('aaa', 'aa')).toBe(1);
    expect(matches('Kalos kalos', 'kal')).toBe(2);
  });

  it('pretty-prints JSON and leaves other text alone', () => {
    expect(pretty('{"a":[1]}')).toBe('{\n  "a": [\n    1\n  ]\n}');
    expect(pretty('not json')).toBe('not json');
  });

  it('spans the messages in guild time', () => {
    const at = ['2026-09-27T14:00:00Z', '2026-09-27T13:40:00Z'];
    expect(timeRange(at, 'Asia/Kuala_Lumpur')).toBe('21:40–22:00');
    expect(timeRange([at[0]!], 'Asia/Kuala_Lumpur')).toBe('22:00');
    expect(timeRange([], 'Asia/Kuala_Lumpur')).toBe('');
  });
});
