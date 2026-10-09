<!-- History → Sign-ins: the stored sign-in audit log of both realms (90
     days), newest first. Admin rows show the client address; member rows
     only a keyed tag of it, never the address. Read only. -->
<script lang="ts">
  import type { SignInEvent, SignInPage, SignInRow } from '@kanade/api-types';
  import { createClient } from '@kanade/client';
  import { LoadError, LoadingState } from '@kanade/ui';
  import { localAt } from './describe';

  let { realm, event, timezone }: { realm: string; event: string; timezone: string } = $props();
  const client = createClient();

  const EVENT: Record<SignInEvent, { label: string; tone: string }> = {
    login_succeeded: { label: 'Signed in', tone: 'status-chip--ok' },
    login_refused: { label: 'Refused', tone: 'status-chip--risk' },
    break_glass_used: { label: 'Break-glass token', tone: 'status-chip--warn' },
    session_ended: { label: 'Session ended', tone: '' },
    session_rotated: { label: 'New address', tone: '' },
    rate_limited: { label: 'Rate limited', tone: 'status-chip--risk' },
    revoke_failed: { label: 'Revoke failed', tone: 'status-chip--warn' },
  };

  let rows = $state<SignInRow[]>([]);
  let nextBefore = $state<number | null>(null);
  let error = $state('');
  let loading = $state(false);
  let loaded = $state(false);

  async function load(more = false) {
    loading = true;
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a request's query, built and sent, never state
    const params = new URLSearchParams();
    if (realm) params.set('realm', realm);
    if (event) params.set('event', event);
    if (more && nextBefore !== null) params.set('before', String(nextBefore));
    try {
      const page = await client.get<SignInPage>(`/api/admin/history/sign-ins${params.size ? `?${params}` : ''}`);
      rows = more ? [...rows, ...page.rows] : page.rows;
      nextBefore = page.next_before;
      error = '';
      loaded = true;
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      loading = false;
    }
  }

  // A changed filter reads the first page again.
  $effect(() => {
    void realm;
    void event;
    void load();
  });
</script>

<div class="history-signins" data-fid="history-signins">
  {#if error && !loaded}<LoadError thing="the sign-ins" reason={error} onretry={() => void load()} level={3} />
  {:else if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
  {#if !loaded && !error}<LoadingState text="Loading the sign-ins…" />{/if}
  {#if loaded && rows.length}
    <div class="history-backups__wrap">
      <table class="history-backups history-signins__table" data-fid="history-signins-table">
        <caption class="vh">Sign-in events, newest first</caption>
        <thead><tr><th scope="col">When</th><th scope="col">Event</th><th scope="col">Who</th><th scope="col">Method · reason</th><th scope="col">From</th><th scope="col">Portal</th></tr></thead>
        <tbody>
          {#each rows as row (row.seq)}
            {@const kind = EVENT[row.event]}
            {@const detail = [row.method, row.reason?.replaceAll('_', ' '), row.request].filter(Boolean).join(' · ')}
            {@const client = row.client && row.realm === 'member' ? `tag ${row.client.slice(0, 8)}` : row.client}
            {@const from = [client, row.device].filter(Boolean).join(' · ')}
            <tr data-fid="history-signin" data-signin={row.seq}>
              <td class="mono">{localAt(row.at, timezone)}</td>
              <td><span class="status-chip {kind.tone}">{kind.label}</span></td>
              <th scope="row">{row.name ?? '—'}</th>
              <td>{detail || '—'}</td>
              <td class="mono" title={row.realm === 'member' ? 'A keyed tag of the address; the address itself is never kept.' : undefined}>{from || '—'}</td>
              <td>{row.realm === 'admin' ? 'Admin' : 'Members'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if loaded && !error}
    <div class="empty history-checkpoints__empty"><strong>No sign-ins match.</strong>Sign-in events are kept for 90 days.</div>
  {/if}
  {#if nextBefore !== null}<div class="history-list-region__pager"><button class="btn" type="button" disabled={loading} onclick={() => void load(true)}>Older sign-ins</button></div>{/if}
</div>
