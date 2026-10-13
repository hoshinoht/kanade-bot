// What the shared roster, strip and estimate components need from the app
// that hosts them (admin Members, the portal's Account › Characters): its
// routes as functions (so the public build never names an admin route), who
// it speaks to (`audience`), and its callbacks. The state classes below read
// through those functions.
import type { BossRow, CharacterLink, CharacterLookup, CharacterRow, EstimateJob, EstimateStarted, EstimateStatus, SettingsActor, Unlinked } from '@kanade/api-types';
import type { Outcome } from '../outcome';

/** Admin wording and actions (Delete, Refresh, link requests), or the member's own (Link opens the notice, Unlink at once). */
export type Audience = 'admin' | 'member';

/**
 * Modules found off this session (no session flag says so): the first generic
 * 404 is remembered and nothing more is drawn or asked. `estimatesGone`: no
 * scouter URL.
 */
export interface ChardataRoutes {
  gone: boolean;
  estimatesGone: boolean;
}

export interface CharacterChange {
  version: number;
  ign?: string;
  world?: string;
  class?: string | null;
  level?: number | null;
  role?: 'main' | 'alt';
  retired?: boolean;
}

export interface CharacterAdd {
  ign: string;
  world: string;
  role: 'main' | 'alt';
  class?: string;
  level?: number | null;
}

/** A create's answer: the row, what the toast adds (an admin's link request), and whether a link can follow. */
export interface Added {
  row: CharacterRow;
  note: string;
  linkable: boolean;
}

export interface LinkApi {
  read(): Promise<Outcome<CharacterLink>>;
  /** Admin: ask the member to link (pending until they confirm). */
  request?(version: number): Promise<Outcome<CharacterLink>>;
  /** Admin: ask NEXON now. */
  refresh?(): Promise<Outcome<CharacterLink>>;
  unlink(): Promise<Outcome<Unlinked | unknown>>;
}

export interface EstimateApi {
  status(): Promise<Outcome<EstimateStatus>>;
  start(refresh: boolean): Promise<Outcome<EstimateStarted>>;
  job(id: string): Promise<Outcome<EstimateJob>>;
}

export interface CharacterApi {
  /** Re-read the list; resolves to the fresh rows. */
  refresh(): Promise<CharacterRow[]>;
  patch(id: string, body: CharacterChange): Promise<Outcome<CharacterRow>>;
  create(body: CharacterAdd): Promise<Outcome<Added>>;
  /** Admin only: members never delete. */
  remove?(id: string): Promise<Outcome<unknown>>;
  /** Whether NEXON is connected for Add; null when the read failed (manual fields). */
  lookupConnected(): Promise<boolean | null>;
  lookUp(ign: string, world: string): Promise<Outcome<CharacterLookup>>;
  link(id: string): LinkApi;
  estimate(id: string): EstimateApi;
}

/** A write the server refused for want of a fresh sign-in (portal): nothing was saved. */
export type Reauth = { kind: 'add' } | { kind: 'edit'; row: CharacterRow } | { kind: 'change'; row: CharacterRow; verb: 'retire' | 'restore' | 'main' };

export interface CharacterKit {
  audience: Audience;
  /** Whose characters, as the words name them: "Mika" (admin), "you" (member). */
  owner: string;
  timeZone: string;
  api: CharacterApi;
  routes: ChardataRoutes;
  /** A History actor's name (who asked for a link, who started a lookup). */
  actor: (who: SettingsActor) => string;
  updatedBy: (row: CharacterRow) => string;
  /** The boss catalog, to name an estimate's bosses. */
  catalog: { readonly data: BossRow[] | null; readonly loading: boolean; load(): unknown };
  /** Admin: where Character data's status lives (Config). */
  configHref?: string;
  /** A lookup step finished (readiness re-reads its chips). */
  onstep?: () => void;
  onreauth?: (ask: Reauth) => void;
  /** After Add's toast (the portal opens the notice for a linkable one). */
  onadded?: (added: Added) => void;
  /** Member: Link…, Review and link…, Link again… open the notice. */
  onlink?: (row: CharacterRow) => void;
  /** Member: Unlink deleted the data at once; the host tells it. */
  onunlinked?: (row: CharacterRow, done: Unlinked) => void;
}

/** Layout-fidelity tags for regions only the portal's boards draw; the admin app passes none. */
export type CharacterFids = Partial<Record<'linkState' | 'next' | 'assumed', string>>;

/** One character's link and NEXON data; null while unread, off or failed (`error`). */
export class CharacterLinkView {
  data = $state<CharacterLink | null>(null);
  error = $state('');
  readonly #api: LinkApi;
  readonly #routes: ChardataRoutes;

  constructor(api: LinkApi, routes: ChardataRoutes) {
    this.#api = api;
    this.#routes = routes;
  }

  async load(): Promise<void> {
    if (this.#routes.gone) return;
    const result = await this.#api.read();
    if (result.ok) {
      this.data = result.value;
      this.error = '';
    } else if (result.unbuilt) this.#routes.gone = true;
    else this.error = result.message;
  }

  /** Link… (admin: pending until the member confirms), Unlink and Refresh; a refusal re-reads. */
  async act(action: 'link' | 'unlink' | 'refresh'): Promise<Outcome<CharacterLink | Unlinked | unknown>> {
    const api = this.#api;
    const result = action === 'link' && api.request ? await api.request(this.data?.version ?? 0) : action === 'refresh' && api.refresh ? await api.refresh() : await api.unlink();
    if (!result.ok && result.unbuilt) this.#routes.gone = true;
    else if (result.ok && action !== 'unlink') this.data = result.value as CharacterLink;
    else await this.load();
    return result;
  }
}

/** The server's clock minus the browser's, learnt from a job this tab started. */
let serverOffset: number | null = null;

const POLL_MS = 1_500;

/** Not finished yet: waiting in line or being looked up. */
export const activeJob = (job: EstimateJob | null): job is EstimateJob => job?.state === 'queued' || job?.state === 'running';

/**
 * One estimate target (a character, or a run's party): its newest job. A
 * queued or running job is read again until it ends; lookups never block one
 * another (a second one waits in line, with its place), and each character's
 * outcome shows as it comes in.
 */
export class EstimateLookup {
  job = $state<EstimateJob | null>(null);
  loaded = $state(false);
  starting = $state(false);
  /** A refused start (422, 429 queue full, a member's 30-minute limit with `nextAt`): what and why. */
  refused = $state<{ code: string; message: string; nextAt: string | null } | null>(null);
  /** Seconds the job has been in its state (waiting, or being looked up); ticks while active. */
  elapsed = $state(0);
  /** Busy (429): the Look up button waits this many seconds. */
  wait = $state(0);
  readonly #api: EstimateApi;
  readonly #routes: ChardataRoutes;
  readonly #onstep: (() => void) | undefined;
  #timer: number | undefined;
  readonly #ticker: number;
  /** Since when (browser clock) the job has been in its state. */
  #since = { key: '', at: 0 };
  /** The job this tab started, so its time starts now. */
  #mine = '';
  /** The finished steps last seen, so each new one is told once. */
  #steps = { id: '', count: 0 };
  #closed = false;

  constructor(api: EstimateApi, routes: ChardataRoutes, onstep?: () => void) {
    this.#api = api;
    this.#routes = routes;
    this.#onstep = onstep;
    this.#ticker = window.setInterval(() => this.#tick(), 1_000);
  }

  async load(): Promise<void> {
    if (this.#routes.gone || this.#routes.estimatesGone) return;
    const result = await this.#api.status();
    if (this.#closed) return;
    if (result.ok) {
      this.#take(result.value.job, false);
      this.#follow();
    } else if (result.unbuilt) this.#routes.estimatesGone = true;
    this.loaded = true;
  }

  async start(refresh: boolean): Promise<void> {
    if (this.starting) return;
    this.starting = true;
    this.refused = null;
    const result = await this.#api.start(refresh);
    this.starting = false;
    if (this.#closed) return;
    if (result.ok) {
      const job = result.value.job;
      serverOffset = Date.parse(job.started_at) - Date.now();
      if (job.id !== this.job?.id) this.#mine = job.id;
      this.#take(job, true);
      this.#follow();
      return;
    }
    if (result.unbuilt) {
      this.#routes.estimatesGone = true;
      return;
    }
    this.refused = { code: result.code ?? '', message: result.message, nextAt: result.nextAt ?? null };
  }

  close(): void {
    this.#closed = true;
    clearTimeout(this.#timer);
    clearInterval(this.#ticker);
  }

  /** Takes a read of the job; `told` says new finished steps (a found or failed character, or the end). */
  #take(job: EstimateJob | null, told: boolean) {
    this.job = job;
    const seconds = job?.state === 'failed' && job.error?.code === 'busy' ? job.error.retry_after_seconds : null;
    this.wait = seconds ?? 0;
    if (!job) return;
    const count = job.progress.done + (activeJob(job) ? 0 : 1);
    if (told && count > (job.id === this.#steps.id ? this.#steps.count : 0)) this.#onstep?.();
    this.#steps = { id: job.id, count };
    this.#clock();
  }

  #tick() {
    if (this.wait > 0) this.wait -= 1;
    this.#clock();
  }

  #clock() {
    const job = this.job;
    if (!activeJob(job)) return;
    const key = `${job.id} ${job.state}`;
    if (this.#since.key !== key) {
      // A state seen change here starts now; one found already under way counts from when it was asked for.
      const seenChange = this.#since.key.startsWith(`${job.id} `) || job.id === this.#mine;
      const asked = serverOffset === null ? Date.now() : Date.parse(job.started_at) - serverOffset;
      this.#since = { key, at: seenChange ? Date.now() : Math.min(Date.now(), asked) };
    }
    this.elapsed = Math.max(0, (Date.now() - this.#since.at) / 1000);
  }

  /** Reads this target's queued or running job again until it ends. */
  #follow() {
    clearTimeout(this.#timer);
    const next = this.job;
    if (!activeJob(next) || this.#closed) return;
    this.#timer = window.setTimeout(async () => {
      const result = await this.#api.job(next.id);
      if (this.#closed) return;
      if (result.ok) {
        if (result.value.id === this.job?.id) this.#take(result.value, true);
        this.#follow();
      } else if (result.unbuilt) this.#routes.estimatesGone = true;
      else if (result.status === 404) await this.load();
    }, POLL_MS);
  }
}
