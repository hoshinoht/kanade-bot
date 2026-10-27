import { describe, expect, it } from 'vitest';
import { rescanProgress } from '../src/extractions/progress';

const TZ = 'Asia/Kuala_Lumpur';
const progress = (job: Parameters<typeof rescanProgress>[0]) => rescanProgress(job, TZ);

describe('rescan progress line', () => {
  it('shows percent, messages and the start in guild time', () => {
    expect(progress({ messages: 27, messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toEqual({ percent: 41, count: '27 of 66 messages', started: 'started 01:40' });
    expect(progress({ messages: 0, messages_total: 1, started_at: '2026-09-29T04:00:00Z' })).toEqual({ percent: 0, count: '0 of 1 message', started: 'started 12:00' });
  });

  it('omits percent and count without a total', () => {
    expect(rescanProgress({ messages: 27, messages_total: null, started_at: '2026-09-28T17:40:00Z' }, TZ)).toEqual({
      percent: null,
      count: '',
      started: 'started 01:40',
    });
    expect(progress({ messages: 27, started_at: '2026-09-28T17:40:00Z' })).toEqual({ percent: null, count: '', started: 'started 01:40' });
    expect(progress({ messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toEqual({ percent: null, count: '', started: 'started 01:40' });
  });

  it('omits percent and count for a zero total', () => {
    expect(progress({ messages: 0, messages_total: 0, started_at: '2026-09-28T17:40:00Z' })).toEqual({ percent: null, count: '', started: 'started 01:40' });
  });

  it('caps the percent at 100 when a read finds more than estimated', () => {
    expect(progress({ messages: 70, messages_total: 66, started_at: '2026-09-28T17:40:00Z' })).toEqual({ percent: 100, count: '70 of 66 messages', started: 'started 01:40' });
  });

  it('omits the start while queued or unreadable', () => {
    const queued = { percent: 41, count: '27 of 66 messages', started: '' };
    expect(progress({ messages: 27, messages_total: 66, started_at: null })).toEqual(queued);
    expect(progress({ messages: 27, messages_total: 66 })).toEqual(queued);
    expect(progress({ messages: 27, messages_total: 66, started_at: 'soon' })).toEqual(queued);
    expect(progress({})).toEqual({ percent: null, count: '', started: '' });
  });
});
