import type { Identity, PublicSession, PublicSessions, PublicStatus, SessionsEnded } from '@kanade/api-types';
import { ApiRequestError, createClient, onUnauthenticated } from '@kanade/client';
import type { Landing } from './landing';

/** Why Sign in is showing, when there is something to say. */
export type SignInNotice = 'failed' | 'switch' | 'signed-out' | { everywhere: number };

/**
 * The one screen the app shows (member-auth-contract §6). Signed out, closed
 * or denied, nothing here reads schedule data.
 */
export type Screen =
  | { kind: 'loading' }
  | { kind: 'closed' }
  | { kind: 'unreachable'; offline: boolean }
  | { kind: 'signin'; notice: SignInNotice | null }
  | { kind: 'denied' }
  | { kind: 'ended' }
  | { kind: 'account'; session: PublicSession };

const client = createClient();

function isError(error: unknown, status: number, code?: string): boolean {
  return error instanceof ApiRequestError && error.status === status && (code === undefined || error.body?.error === code);
}

export class Portal {
  screen = $state<Screen>({ kind: 'loading' });
  /** The bot's name and art for the sign-in windows; decoration only. */
  identity = $state<Identity | null>(null);
  devices = $state<PublicSessions | null>(null);
  devicesError = $state('');
  /** The device being signed out (`handle`), `everywhere`, or `self`. */
  busy = $state('');
  #landing: Landing;

  constructor(landing: Landing) {
    this.#landing = landing;
  }

  /** Status first; open → the session; 401 → Sign in. */
  async load(): Promise<void> {
    if (this.screen.kind !== 'loading') this.screen = { kind: 'loading' };
    let status: PublicStatus;
    try {
      status = await client.get<PublicStatus>('/api/public/status');
    } catch (error) {
      this.#unreachable(error);
      return;
    }
    const landing = this.#landing;
    // The callback's outcome is shown once; checking again starts afresh.
    this.#landing = 'none';
    if (status.portal === 'closed' || landing === 'closed') {
      this.screen = { kind: 'closed' };
      return;
    }
    if (landing === 'denied') {
      this.screen = { kind: 'denied' };
      return;
    }
    try {
      const session = await client.get<PublicSession>('/api/public/session');
      this.screen = { kind: 'account', session };
      void this.loadDevices();
    } catch (error) {
      if (isError(error, 401)) this.screen = { kind: 'signin', notice: landing === 'failed' ? 'failed' : null };
      else if (isError(error, 503, 'closed')) this.screen = { kind: 'closed' };
      else this.#unreachable(error);
    }
  }

  /** The bot identity is public on both origins; a failure renders the monogram. */
  async loadIdentity(): Promise<void> {
    this.identity = await client.get<Identity>('/api/identity').catch(() => null);
  }

  /** A session that ends mid-use (expired, signed out elsewhere, access changed) is the Session ended screen. */
  watch(): () => void {
    onUnauthenticated(() => {
      if (this.screen.kind === 'account') this.screen = { kind: 'ended' };
    });
    return () => onUnauthenticated(null);
  }

  /** Denied → "Use another account": back to Sign in with how to switch. */
  switchAccount(): void {
    this.screen = { kind: 'signin', notice: 'switch' };
  }

  async loadDevices(): Promise<void> {
    try {
      this.devices = await client.get<PublicSessions>('/api/public/sessions');
      this.devicesError = '';
    } catch (error) {
      if (!this.#gone(error)) this.devicesError = error instanceof Error ? error.message : 'The list did not load.';
    }
  }

  /** Sign out one other device. Answers what to tell the member, or null when the screen changed. */
  async endDevice(handle: string, name: string): Promise<{ message: string; ok: boolean } | null> {
    if (this.busy) return null;
    this.busy = handle;
    try {
      await client.delete(`/api/public/sessions/${encodeURIComponent(handle)}`);
      await this.loadDevices();
      return { message: `Signed out ${name}.`, ok: true };
    } catch (error) {
      if (this.#gone(error)) return null;
      await this.loadDevices();
      if (isError(error, 404)) return { message: `${name} was already signed out.`, ok: true };
      return { message: `Couldn't sign out ${name}: ${error instanceof Error ? error.message : 'try again.'}`, ok: false };
    } finally {
      this.busy = '';
    }
  }

  /** Sign out everywhere, this device too; then the signed-out screen. */
  async endEverywhere(): Promise<string | null> {
    if (this.busy) return null;
    this.busy = 'everywhere';
    try {
      const { ended } = await client.post<SessionsEnded>('/api/public/sessions/end-all', {});
      this.#signedOut({ everywhere: ended });
      return null;
    } catch (error) {
      if (this.#gone(error)) return null;
      return `Couldn't sign out everywhere: ${error instanceof Error ? error.message : 'try again.'}`;
    } finally {
      this.busy = '';
    }
  }

  async signOut(): Promise<string | null> {
    if (this.busy) return null;
    this.busy = 'self';
    try {
      await client.post('/api/public/auth/logout', {});
      this.#signedOut('signed-out');
      return null;
    } catch (error) {
      if (this.#gone(error)) return null;
      return `Couldn't sign out: ${error instanceof Error ? error.message : 'try again.'}`;
    } finally {
      this.busy = '';
    }
  }

  #signedOut(notice: SignInNotice): void {
    this.devices = null;
    this.screen = { kind: 'signin', notice };
  }

  /** A closed portal or an ended session replaces the screen (401 arrives through `watch`). */
  #gone(error: unknown): boolean {
    if (isError(error, 503, 'closed')) {
      this.devices = null;
      this.screen = { kind: 'closed' };
      return true;
    }
    return isError(error, 401, 'unauthenticated');
  }

  #unreachable(error: unknown): void {
    const offline = !navigator.onLine || (error instanceof ApiRequestError && error.kind === 'network');
    this.screen = { kind: 'unreachable', offline };
  }
}
