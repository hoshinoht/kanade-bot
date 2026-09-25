import { ApiRequestError, createClient, type Client } from '@kanade/client';

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
      this.error = error instanceof ApiRequestError ? error.message : 'Could not load.';
    } finally {
      this.loading = false;
    }
  }
}

export type Outcome<T = unknown> = { ok: true; value: T } | { ok: false; message: string; status?: number | null };

/** One request with a typed outcome, for forms that keep their input on failure. */
export async function send<T>(work: (client: Client) => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await work(createClient()) };
  } catch (error) {
    if (error instanceof ApiRequestError) return { ok: false, message: error.message, status: error.status };
    return { ok: false, message: 'Something went wrong.' };
  }
}
