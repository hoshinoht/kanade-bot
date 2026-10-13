// The member's own characters on the portal (module `roster`; boards in
// docs/research/2026-10-13-member-characters): their roster rows, links and
// the admins' requests waiting for them (`GET /api/public/me/characters`),
// the NEXON notice and its Link / Cancel, Unlink, My data, their own
// MapleScouter lookups, and the characters of a run they are on. Every route
// is the generic 404 while its module is off: the first one is remembered
// and nothing more is drawn or asked. Kept in memory for this tab only,
// dropped with the session.
import type {
  ApiError,
  CharacterLink,
  CharacterLookup,
  CharacterRow,
  EstimateJob,
  EstimateStarted,
  EstimateStatus,
  LinkCancelled,
  LinkNotice,
  LinkOutcome,
  MyCharacterCreated,
  MyCharacters,
  MyData,
  RunPartyCharacters,
  Unlinked,
  UnlinkedAll,
} from '@kanade/api-types';
import { ApiRequestError, type Client } from '@kanade/client';
import type { CharacterApi, ChardataRoutes, Outcome } from '@kanade/ui';

/** The server's generic 404: a route it does not serve (the module is off), not a missing item. */
const UNBUILT = 'No such endpoint on this origin.';

const ME = '/api/public/me/characters';
const own = (id: string) => `${ME}/${encodeURIComponent(id)}`;

/** "51,402,330": combat power as Discord's `/character data` writes it. */
export const thousands = (value: number) => new Intl.NumberFormat('en-US').format(value);

/** copy.rs `unlinked`: what Unlink deleted, and the shared cards it cleared (the answer counts them, without their channels). */
export function unlinkedWords(done: Unlinked): string {
  if (!done.cards) return `Unlinked ${done.unlinked}. Its NEXON data is deleted. Your roster character is unchanged.`;
  const cards = done.cards === 1 ? '1 shared card now reads' : `${done.cards} shared cards now read`;
  return `Unlinked ${done.unlinked}. Its NEXON data is deleted, and your ${cards} “Character data removed”. Your roster character is unchanged.`;
}

/** copy.rs `unlinked_all`. */
export function unlinkedAllWords(done: UnlinkedAll): string {
  return done.unlinked.length ? `Unlinked everything left: ${done.unlinked.join(', ')}. Nothing from NEXON is kept for you now.` : 'Nothing from NEXON is kept for you.';
}

export class MemberCharacters {
  data = $state<MyCharacters | null>(null);
  /** Why the last read failed; empty once one answers. */
  error = $state('');
  /** `modules.roster` is off: no tab, no run characters. */
  gone = $state(false);
  /** `modules.character_data` (or the scouter URL) found off this session. */
  readonly routes: ChardataRoutes = $state({ gone: false, estimatesGone: false });
  readonly api: CharacterApi;
  #client: Client;
  #gone: (error: unknown) => boolean;
  #loading: Promise<void> | null = null;

  constructor(client: Client, gone: (error: unknown) => boolean) {
    this.#client = client;
    this.#gone = gone;
    const send = <T>(work: (client: Client) => Promise<T>) => this.send(work);
    this.api = {
      refresh: async () => {
        await this.load();
        return this.data?.characters ?? [];
      },
      patch: (id, body) => send((c) => c.patch<CharacterRow>(own(id), body)),
      create: async (body) => {
        const created = await send((c) => c.post<MyCharacterCreated>(ME, body));
        if (!created.ok) return created;
        const { linkable, ...row } = created.value;
        return { ok: true, value: { row, note: '', linkable } };
      },
      // Connected is part of the list: no read of its own.
      lookupConnected: async () => (this.data ? this.data.connected : null),
      lookUp: (ign) => send((c) => c.post<CharacterLookup>(`${ME}/lookup`, { ign })),
      link: (id) => ({
        read: () => send((c) => c.get<CharacterLink>(`${own(id)}/link`)),
        unlink: () => send((c) => c.delete<Unlinked>(`${own(id)}/link`)),
      }),
      estimate: (id) => ({
        status: () => send((c) => c.get<EstimateStatus>(`/api/public/me/estimates?character_id=${encodeURIComponent(id)}`)),
        start: (refresh) => send((c) => c.post<EstimateStarted>('/api/public/me/estimates', { character_id: id, ...(refresh ? { refresh: true } : {}) })),
        job: (job) => send((c) => c.get<EstimateJob>(`/api/public/me/estimates/${encodeURIComponent(job)}`)),
      }),
    };
  }

  /** Read now; a call while a read runs joins it. */
  load(): Promise<void> {
    if (this.gone) return Promise.resolve();
    this.#loading ??= this.#read().finally(() => {
      this.#loading = null;
    });
    return this.#loading;
  }

  async #read(): Promise<void> {
    const result = await this.send((c) => c.get<MyCharacters>(ME));
    if (result.ok) {
      this.data = result.value;
      this.error = '';
      if (!result.value.character_data) this.routes.gone = true;
    } else if (result.unbuilt) this.gone = true;
    else this.error = result.message;
  }

  /** The NEXON notice for one of the member's characters, word for word (v1). */
  notice(id: string): Promise<Outcome<LinkNotice>> {
    return this.send((c) => c.get<LinkNotice>(`${own(id)}/link/notice`));
  }

  /** Link: records the consent to `notice` and runs the first fetch. */
  confirm(id: string, notice: number): Promise<Outcome<LinkOutcome>> {
    return this.send((c) => c.post<LinkOutcome>(`${own(id)}/link`, { notice }));
  }

  /** Cancel: nothing changes, and an admin's request stays. */
  cancel(id: string): Promise<Outcome<LinkCancelled>> {
    return this.send((c) => c.post<LinkCancelled>(`${own(id)}/link/cancel`, {}));
  }

  unlink(id: string): Promise<Outcome<Unlinked>> {
    return this.send((c) => c.delete<Unlinked>(`${own(id)}/link`));
  }

  /** Unlink all under `key`: a 503 recorded nothing, and the same key again finishes the job. */
  unlinkAll(key: string): Promise<Outcome<UnlinkedAll>> {
    return this.send((c) => c.delete<UnlinkedAll>(`${ME}/links`, { idempotencyKey: key }));
  }

  myData(): Promise<Outcome<MyData>> {
    return this.send((c) => c.get<MyData>('/api/public/me/data'));
  }

  /** The characters of a run the member is on; another run is the generic 404 (`not_found`). */
  party(runId: string): Promise<Outcome<RunPartyCharacters>> {
    return this.send((c) => c.get<RunPartyCharacters>(`/api/public/runs/${encodeURIComponent(runId)}/characters`));
  }

  /** One request with a typed outcome; a closed portal or an ended session goes to the portal. */
  async send<T>(work: (client: Client) => Promise<T>): Promise<Outcome<T>> {
    try {
      return { ok: true, value: await work(this.#client) };
    } catch (error) {
      if (!(error instanceof ApiRequestError)) return { ok: false, message: 'Something went wrong.' };
      this.#gone(error);
      const body = error.body as (ApiError & { next_at?: string }) | null;
      return {
        ok: false,
        message: error.message,
        status: error.status,
        code: body?.error ?? null,
        unbuilt: error.status === 404 && body?.message === UNBUILT,
        reauth: error.kind === 'reauth',
        nextAt: body?.next_at ?? null,
      };
    }
  }

  clear(): void {
    this.data = null;
    this.error = '';
    this.gone = false;
    this.routes.gone = false;
    this.routes.estimatesGone = false;
  }
}

/** One run's party characters, read when the member's own run opens; anything but an answer draws nothing. */
export class RunPartyView {
  data = $state<RunPartyCharacters | null>(null);
  readonly #store: MemberCharacters;
  readonly #runId: string;

  constructor(store: MemberCharacters, runId: string) {
    this.#store = store;
    this.#runId = runId;
  }

  async load(): Promise<void> {
    if (this.#store.gone) return;
    const result = await this.#store.party(this.#runId);
    if (result.ok) this.data = result.value;
    else if (result.unbuilt) this.#store.gone = true;
  }
}
