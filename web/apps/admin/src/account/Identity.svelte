<!--
  Account identity (A1, A4, A5): the portrait (Discord avatar, monogram
  fallback; a key for token and Tailscale sign-ins), who and how, the
  diagnostics with Copy, and Sign out at the foot. `compact` is the phone
  strip above the panel; the diagnostics and Sign out then live in the
  Profile tab's "This sign-in" group.
-->
<script lang="ts">
  import type { Me } from '@kanade/api-types';
  import { Avatar, Icon } from '@kanade/ui';
  import { ME_AVATAR } from '../shared/avatar';
  import { methodLong, serverClock } from './account';

  let {
    me,
    timeZone,
    compact = false,
    oncopy,
    onsignout,
  }: { me: Me; timeZone: string; compact?: boolean; oncopy: () => void; onsignout: () => void } = $props();

  const member = $derived(me.member);
  const name = $derived(member?.name ?? me.display);
  const cap = $derived(member ? 'Member' : me.method === 'token' ? 'Access token' : 'Tailscale');
</script>

{#snippet portrait()}
  {#if member}
    <Avatar class="account-id__portrait" src={ME_AVATAR} {name} />
  {:else}
    <span class="account-id__portrait" aria-hidden="true"><Icon name="key" /></span>
  {/if}
{/snippet}

{#if compact}
  <div class="account-strip" data-fid="account-id">
    {@render portrait()}
    <div class="account-strip__who">
      <span class="cap">{cap}</span>
      <h2 class="account-strip__name">{name}</h2>
      <span class="account-strip__method">{methodLong(me.method)}</span>
    </div>
  </div>
{:else}
  <aside class="account-id" aria-label="You" data-fid="account-id">
    <div class="account-id__who">
      {@render portrait()}
      <span class="cap">{cap}</span>
      <h2 class="account-id__name" title={name}>{name}</h2>
      <span class="account-id__method"><Icon name={me.method === 'discord' ? 'users' : 'key'} />{methodLong(me.method)}</span>
    </div>
    <section class="account-id__diag" aria-labelledby="account-diag">
      <h3 class="cap" id="account-diag">Diagnostics</h3>
      <dl class="account-id__dl">
        <dt>User id</dt>
        <dd>{member?.id ?? '—'}</dd>
        <dt>Method</dt>
        <dd>{me.method}</dd>
        <dt>Version</dt>
        <dd>{me.version}</dd>
        <dt>Server</dt>
        <dd>{serverClock(me.server_time, timeZone)}</dd>
      </dl>
      <button type="button" class="btn account-full" onclick={oncopy}><Icon name="copy" />Copy diagnostics</button>
    </section>
    <div class="account-id__foot">
      <button type="button" class="btn btn--danger account-full" onclick={onsignout}><Icon name="log-out" />Sign out</button>
    </div>
  </aside>
{/if}
