import { ApiRequestError, createClient, type Client } from '@kanade/client';

/** The server's generic 404 for a route it does not serve (an unbuilt feature, not a missing item). */
const UNBUILT = 'No such endpoint on this origin.';

/** An error in words; a route this server has not built yet is not an outage. */
export function errorText(error: ApiRequestError): string {
  if (error.status === 404 && error.body?.message === UNBUILT) return "This isn't available on this server yet.";
  return error.message;
}

/** A page's read model: loaded when the page opens, reloaded after its own edits. */
export class Resource<T> {
  data = $state<T | null>(null);
  error = $state('');
  loading = $state(false);
  #path: string;
  #client: Client;

  constructor(path: string, client: Client = createClient()) {
    this.#path = path;
    this.#client = client;
  }

  async load(): Promise<void> {
    this.loading = true;
    try {
      this.data = await this.#client.get<T>(this.#path);
      this.error = '';
    } catch (error) {
      this.error = error instanceof ApiRequestError ? errorText(error) : 'Could not load.';
    } finally {
      this.loading = false;
    }
  }
}

export type Outcome<T = unknown> = { ok: true; value: T } | { ok: false; message: string; status?: number | null; code?: string | null };

/** One request with a typed outcome, for forms that keep their input on failure. */
export async function send<T>(work: (client: Client) => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await work(createClient()) };
  } catch (error) {
    if (error instanceof ApiRequestError) return { ok: false, message: errorText(error), status: error.status, code: error.body?.error ?? null };
    return { ok: false, message: 'Something went wrong.' };
  }
}
