// The weekly timings the signed-in member is on (`GET /api/public/timings`)
// and their ownership writes. Read when My runs opens and after every write;
// kept in memory for this tab only, dropped with the session.
import type { MemberTimings } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import { MemberList } from '../memberList.svelte';
import { writePath, type OwnWrite } from './ownership';

/**
 * A write's outcome: done; `reauth` (the owner change needs a fresh Discord
 * sign-in, nothing saved); refused or failed with the error; or null when
 * the session ended or the portal closed (the portal replaces the screen).
 */
export type WriteOutcome = { done: true } | { done: false; reauth: true } | { done: false; reauth: false; error: unknown } | null;

export class MemberTimingsList extends MemberList<MemberTimings> {
  /** The write in flight (its path), so its buttons show busy and others wait. */
  busy = $state('');

  constructor(client: Client, gone: (error: unknown) => boolean) {
    super(client, gone, '/api/public/timings', 'Your weekly timings did not load.');
  }

  /** One write at a time; the list is read again after anything but a fresh-sign-in refusal. */
  async write(write: OwnWrite): Promise<WriteOutcome> {
    if (this.busy) return null;
    const path = writePath(write);
    this.busy = path;
    try {
      await this.client.post(path, write.kind === 'hand' ? { to: write.to.id } : {});
      await this.load();
      return { done: true };
    } catch (error) {
      if (this.gone(error)) return null;
      if (error instanceof ApiRequestError && error.kind === 'reauth') return { done: false, reauth: true };
      await this.load();
      return { done: false, reauth: false, error };
    } finally {
      this.busy = '';
    }
  }
}
