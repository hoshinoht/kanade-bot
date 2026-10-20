import type { SecretCheck, SecretName, SetupSecrets } from '@kanade/api-types';
import { ApiRequestError, createClient, type Client } from '@kanade/client';

/**
 * Setup's saved secrets (the `setup/secrets` routes): which are set and
 * where, entering one (check, then save; write-only, the value lives only in
 * the dialog), and "Restart now". `available` is false on a server with no
 * saved directory (409 `not_saveable`), which hides the group.
 */
export class SetupSecretsState {
  view = $state<SetupSecrets | null>(null);
  available = $state(true);
  /** The dialog's secret; `null` while closed. */
  editing = $state<SecretName | null>(null);
  value = $state('');
  check = $state<SecretCheck | null>(null);
  busy = $state(false);
  error = $state('');
  /** The save needs a fresh sign-in (401 `admin_reauth_required`). */
  reauth = $state(false);
  restarting = $state(false);
  restartError = $state('');

  readonly #client: Client;

  constructor(client: Client = createClient()) {
    this.#client = client;
  }

  async load(): Promise<void> {
    try {
      this.view = await this.#client.get<SetupSecrets>('/api/admin/setup/secrets');
      this.available = true;
    } catch (error) {
      if (error instanceof ApiRequestError && error.status === 409) this.available = false;
    }
  }

  open(name: SecretName): void {
    this.editing = name;
    this.value = '';
    this.check = null;
    this.error = '';
    this.reauth = false;
  }

  close(): void {
    this.editing = null;
    this.value = '';
    this.check = null;
  }

  async runCheck(): Promise<void> {
    const name = this.editing;
    if (!name || !this.value.trim() || this.busy) return;
    this.busy = true;
    this.error = '';
    try {
      this.check = await this.#client.post<SecretCheck>(`/api/admin/setup/secrets/${name}/check`, { value: this.value });
    } catch (error) {
      this.error = message(error);
    } finally {
      this.busy = false;
    }
  }

  /** Saves the dialog's value; `true` once saved (the dialog then closes). */
  async save(): Promise<boolean> {
    const name = this.editing;
    if (!name || !this.value.trim() || this.busy) return false;
    this.busy = true;
    this.error = '';
    this.reauth = false;
    try {
      this.view = await this.#client.put<SetupSecrets>(`/api/admin/setup/secrets/${name}`, { value: this.value });
      this.close();
      return true;
    } catch (error) {
      if (error instanceof ApiRequestError && error.kind === 'reauth') this.reauth = true;
      else this.error = message(error);
      return false;
    } finally {
      this.busy = false;
    }
  }

  /** `true` once the server accepted the restart (it exits shortly after). */
  async restart(): Promise<boolean> {
    if (this.restarting) return false;
    this.restarting = true;
    this.restartError = '';
    try {
      await this.#client.post<null>('/api/admin/setup/restart', {});
      return true;
    } catch (error) {
      this.restarting = false;
      if (error instanceof ApiRequestError && error.kind === 'reauth') this.reauth = true;
      this.restartError = message(error);
      return false;
    }
  }
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : 'Something went wrong.';
}
