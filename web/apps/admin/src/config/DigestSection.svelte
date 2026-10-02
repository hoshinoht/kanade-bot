<script lang="ts">
  import type { Channel } from '@kanade/api-types';
  import { Modal, type Toaster } from '@kanade/ui';
  import SettingsPanel from './SettingsPanel.svelte';
  import { Resource, send } from '../resource.svelte';

  let { toaster }: { toaster: Toaster } = $props();
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
    if (result.ok) toaster.show({ message: result.value.message, tone: 'ok' });
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
    <label class="field digest__channel"
      ><span>Channel</span>
      <select bind:value={channel}>
        <option value="">The digest channel (env)</option>
        {#each channels.data ?? [] as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
      </select>
    </label>
    <button class="btn btn--primary settings__key" type="submit" aria-disabled={busy}>Post it now…</button>
  </form>
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <p class="settings__box">
    It follows clears and answers through the week and replaces an earlier digest for the same week. “Post it now…” asks first.
  </p>
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

  .digest__channel select {
    width: 100%;
  }
</style>
