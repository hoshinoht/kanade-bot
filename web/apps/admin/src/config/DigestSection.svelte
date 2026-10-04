<script lang="ts">
  import type { Channel, LastDigest } from '@kanade/api-types';
  import { Icon, Modal, Select, weekStartLabel, type Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import SettingsPanel from './SettingsPanel.svelte';
  import { Resource, send } from '../resource.svelte';

  let {
    toaster,
    last = null,
    onposted,
  }: { toaster: Toaster; last?: LastDigest | null; onposted?: () => unknown } = $props();
  const uid = $props.id();

  const channels = new Resource<Channel[]>('/api/admin/channels');
  $effect(() => {
    void channels.load();
  });
  let week = $state<'this' | 'next'>('this');
  let channel = $state('');
  let busy = $state(false);
  let error = $state('');
  let asking = $state(false);
  const channelName = $derived(channel ? (channels.data?.find((c) => c.id === channel)?.name ?? channel) : 'the digest channel');
  // `posted_at` already carries the guild's offset, so its own date and clock are guild time.
  const lastAt = $derived(last ? `${weekStartLabel(last.posted_at)} ${last.posted_at.slice(11, 16)}` : '');
  const lastWeek = $derived(last ? (last.this_week ? 'this week' : `week of ${weekStartLabel(last.week_start)}`) : '');

  function ask(event: SubmitEvent) {
    event.preventDefault();
    if (!busy) asking = true;
  }

  async function post() {
    if (busy) return;
    busy = true;
    const result = await send((c) => c.post<{ message: string }>('/api/admin/digest', { week, channel_id: channel || null }));
    busy = false;
    error = result.ok ? '' : result.message;
    if (result.ok) {
      toaster.show({ message: result.value.message, tone: 'ok' });
      void onposted?.();
    }
  }
</script>

<SettingsPanel title="Weekly digest">
  {#snippet lead()}Posts the whole guild’s week, naming people rather than pinging them.{/snippet}
  <form class="settings__card settings__card--row digest" data-fid="cfg-card" onsubmit={ask}>
    <div class="field">
      <span id="{uid}-week">Week</span>
      <div class="seg" role="group" aria-labelledby="{uid}-week">
        <button type="button" aria-pressed={week === 'this'} onclick={() => (week = 'this')}>This week</button>
        <button type="button" aria-pressed={week === 'next'} onclick={() => (week = 'next')}>Next week</button>
      </div>
    </div>
    <div class="field digest__channel"
      ><span>Channel</span>
      <Select
        label="Channel"
        bind:value={channel}
        options={[{ value: '', label: 'The digest channel (env)' }, ...(channels.data ?? []).map((c) => ({ value: c.id, label: c.name }))]}
        noun="channels"
      />
    </div>
    <button class="btn btn--primary settings__key" type="submit" aria-disabled={busy}>Post it now…</button>
  </form>
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <p class="settings__box">
    It follows clears and answers through the week and replaces an earlier digest for the same week. “Post it now…” asks first.
  </p>
  {#if last}
    <section class="settings__card digest__last" data-fid="cfg-card" aria-labelledby="{uid}-last">
      <h4 class="cap" id="{uid}-last">Last posted</h4>
      <p class="digest__lastline">
        <b class="mono">{lastAt}</b> · {lastWeek} · {last.channel_name ?? last.channel_id}{last.url ? ' · ' : ''}{#if last.url}<a
            href={last.url}
            target="_blank"
            rel="noopener noreferrer">open in Discord<Icon name="external-link" /><span class="vh"> (opens in a new tab)</span></a
          >{/if}
      </p>
    </section>
  {/if}
</SettingsPanel>

<Modal bind:open={asking} title="Post {week === 'this' ? "this week's" : "next week's"} digest to {channelName}?" narrow>
  <p>It replaces an earlier digest for the same week.</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button
      class="btn btn--primary"
      type="button"
      onclick={() => {
        close();
        void post();
      }}>Post it now</button
    >
  {/snippet}
</Modal>

<style>
  .digest__channel {
    flex: 1 1 auto;
  }

  .digest__last {
    gap: 6px;
  }

  .digest__last h4 {
    margin: 0;
  }

  .digest__lastline {
    margin: 0;
    font-size: var(--fs-body);
  }

  .digest__lastline a {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }
</style>
