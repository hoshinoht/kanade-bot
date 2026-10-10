import { describe, expect, it } from 'vitest';
import { progressLine, rescanProgress } from '../src/extractions/progress';

const TZ = 'Asia/Kuala_Lumpur';
const line = (job: Parameters<typeof rescanProgress>[0]) => progressLine(rescanProgress(job, TZ));

describe('rescan progress line', () => {
  it('shows percent, messages and the start in guild time', () => {
    expect(line({ messages: 27, messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toBe('41% · 27 of 66 messages · started 01:40');
    expect(line({ messages: 0, messages_total: 1, started_at: '2026-09-29T04:00:00Z' })).toBe('0% · 0 of 1 message · started 12:00');
  });

  it('omits percent and count without a total', () => {
    expect(rescanProgress({ messages: 27, messages_total: null, started_at: '2026-09-28T17:40:00Z' }, TZ)).toEqual({
      percent: null,
      count: '',
      started: 'started 01:40',
    });
    expect(line({ messages: 27, started_at: '2026-09-28T17:40:00Z' })).toBe('started 01:40');
    expect(line({ messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toBe('started 01:40');
  });

  it('omits percent and count for a zero total', () => {
    expect(line({ messages: 0, messages_total: 0, started_at: '2026-09-28T17:40:00Z' })).toBe('started 01:40');
  });

  it('caps the percent at 100 when a read finds more than estimated', () => {
    expect(line({ messages: 70, messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toBe('100% · 70 of 66 messages · started 01:40');
  });

  it('omits the start while queued or unreadable', () => {
    expect(line({ messages: 27, messages_total: 66, started_at: null })).toBe('41% · 27 of 66 messages');
    expect(line({ messages: 27, messages_total: 66 })).toBe('41% · 27 of 66 messages');
    expect(line({ messages: 27, messages_total: 66, started_at: 'soon' })).toBe('41% · 27 of 66 messages');
    expect(line({})).toBe('');
  });
});
