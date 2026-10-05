<!--
  Account (A02): who is signed in, from `GET /api/admin/me`. A Discord sign-in
  shows the member's chatbot access, bossing role, server roles (names, never
  ids) and the chat allowance row Limits shows; the admin token and Tailscale
  have no Discord account, so they get one neutral note instead.
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import '@kanade/ui/styles/members.scss';
  import type { Me } from '@kanade/api-types';
  import { LoadError, LoadingState, StateNote } from '@kanade/ui';
  import Name from '../names/Name.svelte';
  import { Resource } from '../resource.svelte';

  const me = new Resource<Me>('/api/admin/me');
  $effect(() => void me.load());

  const METHOD: Record<string, string> = { discord: 'Discord', tailscale: 'Tailscale', token: 'the admin token' };
  const ACCESS = { staff: 'Staff — exempt from chatbot budgets', pilot: 'Chat pilot', none: 'No chatbot access' };
  const method = $derived(me.data ? (METHOD[me.data.method] ?? me.data.method) : '');
  const member = $derived(me.data?.member ?? null);
  const allowance = $derived(member?.allowance ?? null);
</script>

<PageLine>
  <h1>{me.data?.display ?? 'Account'}</h1>
  {#if method}<p class="pageline__context">signed in with {method}</p>{/if}
</PageLine>
<PaneWindow title="Account">
  {#if me.error}
    <LoadError thing="your account" reason={me.error} onretry={() => void me.load()} />
  {:else if !me.data}
    <LoadingState text="Loading your account…" />
  {:else if !member}
    <StateNote icon="shield" title={me.data.method === 'discord' ? 'Not in the member list' : 'Not a Discord member'}>
      {#if me.data.method === 'discord'}
        Your Discord account has no member row right now, so there are no server roles or chat allowance to show.
      {:else}
        Signed in with {method}: there is no Discord account, server roles or chat allowance to show.
      {/if}
    </StateNote>
  {:else}
    <dl class="membersheet__grid account__facts" data-fid="account-facts">
      <dt>Discord account</dt>
      <dd><Name kind="member" id={member.id} name={member.name} /></dd>
      <dt>Chatbot</dt>
      <dd>{ACCESS[member.access]}</dd>
      <dt>Bossing role</dt>
      <dd>{member.bossing ? 'Yes — on the roster' : 'No — not on the roster'}</dd>
      <dt>Server roles</dt>
      <dd>
        {#if member.roles === null}
          <span class="note">Unavailable while Discord is disconnected.</span>
        {:else if member.roles.length}
          <ul class="account__roles" aria-label="Server roles">
            {#each member.roles as role (role.id)}<li><Name kind="role" id={role.id} name={role.name} /></li>{/each}
          </ul>
        {:else}
          None
        {/if}
      </dd>
      <dt>Chat allowance</dt>
      <dd>
        {#if !allowance}
          No chatbot access
        {:else if !allowance.allowance}
          Exempt (staff)
        {:else}
          <span class="mono">{allowance.allowance.count} per {allowance.allowance.per_s}s</span>
          · {#if allowance.used}<b>{allowance.used} used</b>, {allowance.allowance.count - allowance.used} left{:else}idle this window{/if}
          {#if allowance.override}<span class="note">· own allowance</span>{/if}
        {/if}
      </dd>
    </dl>
  {/if}
</PaneWindow>

<style>
  .account__facts {
    padding: 4px 2px;
  }

  .account__roles {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
