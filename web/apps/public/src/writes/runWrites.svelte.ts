// The member's answer and move on their own runs (`PUT /api/public/runs/{id}/answer`,
// `POST /api/public/runs/{id}/move`). An answer shows at once and rolls back
// when refused; a `409 stale` re-reads the weeks and says what changed; a
// `401 reauth_required` saves nothing and leaves "Confirm it's you" to the page.
import type { MemberMoveResult, MemberRunLink, MemberRunResult } from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import type { Slot } from '@kanade/ui';
import type { MemberWeeks } from '../weeks.svelte';
import { changeWords, withAnswer, type Choice } from './runs';

/**
 * How a write ended: done (with the server's answer); `reauth` (nothing
 * saved, a fresh sign-in is needed); `stale` with what changed meanwhile;
 * `refused` with the server's reason; or null when the session ended or the
 * portal closed (the portal replaces the screen).
 */
export type RunOutcome<T> = { kind: 'done'; result: T } | { kind: 'reauth' } | { kind: 'stale'; words: string } | { kind: 'refused'; words: string; code: string } | null;

export class RunWrites {
  /** The run a write is out for, so its controls show busy and others wait. */
  busy = $state('');
  #client: Client;
  #weeks: MemberWeeks;
  #gone: (error: unknown) => boolean;

  constructor(client: Client, weeks: MemberWeeks, gone: (error: unknown) => boolean) {
    this.#client = client;
    this.#weeks = weeks;
    this.#gone = gone;
  }

  /** The caller's answer, shown at once; the weeks keep it only if the server does. */
  async answer(id: string, memberId: string, answer: Choice): Promise<RunOutcome<MemberRunResult>> {
    const found = this.#weeks.find(id);
    if (!found || this.busy) return null;
    const before = found.run;
    this.#weeks.put(withAnswer(before, memberId, answer));
    return this.#write(id, before, memberId, () =>
      this.#client.put<MemberRunResult>(`/api/public/runs/${encodeURIComponent(id)}/answer`, { answer, version: found.week.version }),
    );
  }

  /** This week only: the run to `slot` (an own-time run keeps its clock on a null time). */
  async move(id: string, memberId: string, slot: Slot): Promise<RunOutcome<MemberMoveResult>> {
    const found = this.#weeks.find(id);
    if (!found || this.busy) return null;
    return this.#write(id, found.run, memberId, () =>
      this.#client.post<MemberMoveResult>(`/api/public/runs/${encodeURIComponent(id)}/move`, { day: slot.day, time: slot.time, version: found.week.version }),
    );
  }

  /**
   * The run a Discord link names (`GET /api/public/runs/{id}`), as the
   * caller may see it now: `missing` when it is not theirs to see (404),
   * null when the session ended or the portal closed.
   */
  async link(id: string): Promise<{ kind: 'ok'; link: MemberRunLink } | { kind: 'missing' } | { kind: 'failed'; words: string } | null> {
    try {
      return { kind: 'ok', link: await this.#client.get<MemberRunLink>(`/api/public/runs/${encodeURIComponent(id)}`) };
    } catch (error) {
      if (this.#gone(error)) return null;
      if (error instanceof ApiRequestError && error.status === 404) return { kind: 'missing' };
      return { kind: 'failed', words: error instanceof Error ? error.message : 'The run did not load.' };
    }
  }

  async #write<T extends MemberRunResult>(id: string, before: MemberRunResult['run'], memberId: string, send: () => Promise<T>): Promise<RunOutcome<T>> {
    this.busy = id;
    try {
      const result = await send();
      this.#weeks.put(result.run, result.version);
      return { kind: 'done', result };
    } catch (error) {
      this.#weeks.put(before);
      if (this.#gone(error)) return null;
      if (error instanceof ApiRequestError && error.kind === 'reauth') return { kind: 'reauth' };
      const code = error instanceof ApiRequestError ? (error.body?.error ?? '') : '';
      if (code === 'stale') {
        await this.#weeks.refresh();
        const now = this.#weeks.find(id);
        return { kind: 'stale', words: changeWords(before, now?.run ?? null, now?.week ?? this.#weeks.this ?? { days: [] }, memberId) };
      }
      // Anything else refused leaves the run as the server has it now.
      void this.#weeks.refresh();
      return { kind: 'refused', words: error instanceof Error ? error.message : 'Try again.', code };
    } finally {
      this.busy = '';
    }
  }
}
