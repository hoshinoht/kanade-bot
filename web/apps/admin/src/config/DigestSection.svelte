<script lang="ts">
  import type { Channel } from '@kanade/api-types';
  import type { Toaster } from '@kanade/ui';
  import { Resource, send } from '../resource.svelte';

  let { toaster }: { toaster: Toaster } = $props();

  const channels = new Resource<Channel[]>('/api/admin/channels');
  $effect(() => {
    void channels.load();
  });
  let week = $state<'this' | 'next'>('this');
  let channel = $state('');
  let busy = $state(false);
  let error = $state('');

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    busy = true;
    const result = await send((c) => c.post<{ message: string }>('/api/admin/digest', { week, channel_id: channel || null }));
    busy = false;
    error = result.ok ? '' : result.message;
    if (result.ok) toaster.show({ message: result.value.message, tone: 'ok' });
  }
</script>

<h3 class="settings__title">Weekly digest</h3>
<form class="filters" onsubmit={submit}>
  <label class="field"
    ><span>Week</span>
    <select bind:value={week}><option value="this">This week</option><option value="next">Next week</option></select>
  </label>
  <label class="field"
    ><span>Channel</span>
    <select bind:value={channel}>
      <option value="">The digest channel (env)</option>
      {#each channels.data ?? [] as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
    </select>
  </label>
  <button class="btn btn--primary" type="submit" aria-disabled={busy}>Post it now</button>
</form>
<p class="field__error" role="alert">{error}</p>
<p class="note">
  Posts the whole guild's week. It names people rather than pinging them, follows clears and answers through the week, and replaces an
  earlier digest for the same week.
</p>
