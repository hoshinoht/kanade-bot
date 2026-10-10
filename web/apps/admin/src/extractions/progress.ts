import type { RescanJob } from '@kanade/api-types';
import { clockTime } from '@kanade/ui';

/** A running re-read's message progress; each part is empty when the API did not supply it (spec O8). */
export interface RescanProgress {
  /** 0–100, or null without a known, non-zero total. */
  percent: number | null;
  /** "27 of 66 messages", or '' without a known, non-zero total. */
  count: string;
  /** "started 01:40" in the guild's zone, or '' while queued. */
  started: string;
}

export function rescanProgress(job: Pick<RescanJob, 'messages' | 'messages_total' | 'started_at'>, timeZone: string): RescanProgress {
  const read = job.messages;
  const total = job.messages_total;
  const known = typeof read === 'number' && typeof total === 'number' && total > 0;
  // A backfill can leave a read channel above its estimate: never past 100%.
  const percent = known ? Math.min(100, Math.max(0, Math.round((read / total) * 100))) : null;
  const count = known ? `${read} of ${total} ${total === 1 ? 'message' : 'messages'}` : '';
  const at = job.started_at && !Number.isNaN(Date.parse(job.started_at)) ? clockTime(job.started_at, timeZone, false) : '';
  return { percent, count, started: at ? `started ${at}` : '' };
}

/** "41% · 27 of 66 messages · started 01:40", leaving out what is missing. */
export function progressLine(p: RescanProgress): string {
  return [p.percent === null ? '' : `${p.percent}%`, p.count, p.started].filter(Boolean).join(' · ');
}
