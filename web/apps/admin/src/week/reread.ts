import type { RescanJob } from '@kanade/api-types';
import { ApiRequestError, createClient } from '@kanade/client';
import type { MoveOutcome } from '../store.svelte';

const POLL_MS = 1000;
const MAX_POLLS = 90;
/** Transient poll failures tolerated in a row before giving up on tracking. */
const MAX_POLL_ERRORS = 5;

export interface RescanTransport {
  post<T>(path: string, body: unknown): Promise<T>;
  get<T>(path: string): Promise<T>;
}

/**
 * v4's per-run "re-read this channel" (a one-channel rescan over this boss
 * week), awaited to its end so the caller can report one outcome. A poll
 * error is not a failure while the job continues: polling goes on, and only
 * consecutive failures give up on tracking (the job itself keeps running).
 */
export async function reread(
  channelId: string,
  channelName: string,
  {
    wait = (ms: number) => new Promise((r) => setTimeout(r, ms)),
    transport,
  }: { wait?: (ms: number) => Promise<void>; transport?: RescanTransport } = {},
): Promise<MoveOutcome> {
  const request: RescanTransport = transport ?? createClient();
  const lost = (why: string): MoveOutcome => ({ ok: false, message: `Couldn't re-read ${channelName}: ${why}` });
  let job: RescanJob;
  try {
    job = await request.post<RescanJob>('/api/admin/rescan', { channels: [channelId], window: 'week' });
  } catch (error) {
    return lost(error instanceof ApiRequestError ? error.message : 'something went wrong.');
  }
  let errors = 0;
  for (let i = 0; job.state === 'running' && i < MAX_POLLS; i++) {
    await wait(POLL_MS);
    try {
      job = await request.get<RescanJob>(`/api/admin/rescan/${encodeURIComponent(job.id)}`);
      errors = 0;
    } catch (error) {
      // The job is still running server-side; only a vanished job ends this.
      if (error instanceof ApiRequestError && error.status === 404)
        return { ok: false, message: `The re-read of ${channelName} disappeared; start it again from Extractions.` };
      errors += 1;
      if (errors > MAX_POLL_ERRORS)
        return { ok: false, message: `Lost track of the re-read of ${channelName}; it continues on Extractions.` };
    }
  }
  if (job.state === 'running') return { ok: false, message: `Still re-reading ${channelName}; see Extractions for progress.` };
  if (job.state !== 'done') return { ok: false, message: `The re-read of ${channelName} was cancelled.` };
  const messages = job.channels.reduce((n, c) => n + c.messages, 0);
  const changes = job.proposals ? `${job.proposals} change${job.proposals === 1 ? '' : 's'} proposed in the Inbox` : 'nothing to change';
  return { ok: true, message: `Re-read ${channelName}: ${messages} message${messages === 1 ? '' : 's'}, ${changes}.` };
}
