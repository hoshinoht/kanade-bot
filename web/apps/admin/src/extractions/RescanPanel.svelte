<!--
  Re-read the party channels (v4 rescan_job.html). The job is polled with the
  bounded poller instead of v4's self-replacing htmx fragment, and the
  progress line is a polite live region.
-->
<script lang="ts">
  import type { Channel, RescanJob } from '@kanade/api-types';
  import { createClient, createPoller } from '@kanade/client';
  import { send } from '../resource.svelte';

  let { targets }: { targets: Channel[] } = $props();

  let chosen = $state<string[]>([]);
  let window_ = $state<RescanJob['window']>('week');
  let job = $state<RescanJob | null>(null);
  let error = $state('');
  const client = createClient();

  const poller = createPoller<RescanJob>({
    task: (signal) => client.get<RescanJob>(`/api/admin/rescan/${encodeURIComponent(job!.id)}`, { signal }),
    intervalMs: 1000,
    maxFailures: 3,
    onData: (next) => {
      job = next;
      if (next.state !== 'running') poller.stop();
    },
    onError: () => (error = 'Lost track of the rescan; refresh to see where it got to.'),
  });
  $effect(() => () => poller.stop());

  const done = $derived(job ? job.channels.filter((c) => c.state === 'done').length : 0);

  async function start(event: SubmitEvent) {
    event.preventDefault();
    error = '';
    const result = await send((c) => c.post<RescanJob>('/api/admin/rescan', { channels: chosen, window: window_ }));
    if (!result.ok) {
      error = result.message;
      return;
    }
    job = result.value;
    poller.start();
  }

  async function cancel() {
    if (!job) return;
    const id = job.id;
    poller.stop();
    const result = await send((c) => c.delete<RescanJob>(`/api/admin/rescan/${encodeURIComponent(id)}`));
    if (result.ok) job = result.value;
  }
</script>

<form class="rescan" onsubmit={start}>
  <fieldset class="field">
    <legend class="label">Channels</legend>
    <div class="run__people">
      {#each targets as t (t.id)}
        <label class="chip"><input type="checkbox" value={t.id} bind:group={chosen} /> {t.name}</label>
      {/each}
    </div>
  </fieldset>
  <div class="formrow">
    <label class="field">
      <span>Window</span>
      <select bind:value={window_}>
        <option value="week">This boss week</option>
        <option value="since_reset">Since the last reset</option>
        <option value="two_weeks">The last two weeks</option>
      </select>
    </label>
    <button class="btn btn--primary" type="submit" disabled={job?.state === 'running'}>Re-read</button>
    {#if job?.state === 'running'}<button class="btn" type="button" onclick={() => void cancel()}>Cancel</button>{/if}
  </div>
  <p class="field__error" role="alert">{error}</p>
  <p class="rescan__status" role="status">
    {#if job}
      {#if job.state === 'running'}Reading {done} of {job.channels.length} channels…
      {:else if job.state === 'done'}Done: {job.channels.length} channels read, {job.proposals} change{job.proposals === 1 ? '' : 's'} proposed.
      {:else}Cancelled after {done} of {job.channels.length} channels.{/if}
    {/if}
  </p>
  {#if job}
    <ul class="rescan__list">
      {#each job.channels as c (c.id)}
        <li><span>{c.name}</span> <span class="status status--{c.state === 'done' ? 'confirmed' : c.state === 'reading' ? 'planned' : 'waiting'}">{c.state}</span>
          {#if c.state === 'done'}<span class="id">{c.messages} messages</span>{/if}</li>
      {/each}
    </ul>
  {/if}
</form>

<style>
  .rescan__list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.2rem;
  }

  .field__error:empty {
    display: none;
  }
</style>
