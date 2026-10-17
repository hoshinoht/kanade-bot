// The member's own preferences (`GET`/`PATCH /api/public/me/preferences`):
// the "OK to be teased" opt-in, the admins' running-joke note (read-only)
// and whether teasing is on for everyone. Read when Account opens; the
// opt-in stays editable while teasing is off and needs no fresh sign-in.
import type { MyPreferences } from '@kanade/api-types';
import type { Client } from '@kanade/client';
import { MemberList } from '../memberList.svelte';

export class MemberPreferences extends MemberList<MyPreferences> {
  /** A write is out. */
  busy = $state(false);

  constructor(client: Client, gone: (error: unknown) => boolean) {
    super(client, gone, '/api/public/me/preferences', 'Your preferences did not load.');
  }

  /** Set the opt-in. Answers '' once saved, the reason it wasn't, or null when the session ended or the portal closed. */
  async setTease(on: boolean): Promise<string | null> {
    if (this.busy) return null;
    this.busy = true;
    try {
      this.data = await this.client.patch<MyPreferences>('/api/public/me/preferences', { tease_ok: on });
      return '';
    } catch (error) {
      if (this.gone(error)) return null;
      return error instanceof Error ? error.message : 'Try again.';
    } finally {
      this.busy = false;
    }
  }
}
