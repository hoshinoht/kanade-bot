<!--
  Re-read the party channels (v4 rescan_job.html; M3E B_CfgReread). Channel
  checkbox chips and the window choice in one card; a running job gets its
  own card with progress (messages read of the total and the start time, each
  only when the API supplies it), what it found so far and Cancel. The job is polled
  with the bounded poller instead of v4's self-replacing htmx fragment, and
  its progress line is repeated in a polite live region.
-->
<script lang="ts">
  import type { Channel, RescanJob } from '@kanade/api-types';
  import { createClient, createPoller } from '@kanade/client';
  import { LiveRegion, MultiSelect, Select, WavyProgress } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { tick } from 'svelte';
  import { send } from '../resource.svelte';
  import { getChrome } from '../shell/chrome';
  import { rescanProgress } from './progress';

  let { targets, details = false }: { targets: Channel[]; /** Link the job card to Extractions (Config). */ details?: boolean } = $props();

  const uid = $props.id();
  let chosen = $state<string[]>([]);
  let window_ = $state<RescanJob['window']>('week');
  let job = $state<RescanJob | null>(null);
  let error = $state('');
  let go: HTMLButtonElement | undefined = $state();
  const client = createClient();

  /** Pre-ticks these channels (Extractions' "Re-read this channel"); the window and Re-read stay the admin's. */
  export function choose(ids: string[]) {
    chosen = ids.filter((id) => targets.some((t) => t.id === id));
  }

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

  const chrome = getChrome();
  const running = $derived(job?.state === 'running');
  const done = $derived(job ? job.channels.filter((c) => c.state === 'done').length : 0);
  const total = $derived(job?.channels.length ?? 0);
  // While running, progress is in messages and only what the API supplies; a finished job keeps its channel tally.
  const progress = $derived(job && running ? rescanProgress(job, chrome?.timezone || 'Asia/Kuala_Lumpur') : null);
  const percent = $derived(progress ? progress.percent : total ? Math.round((done / total) * 100) : 0);
  const read = $derived(job ? job.channels.reduce((sum, c) => sum + (c.state === 'done' ? c.messages : 0), 0) : 0);
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
  const status = $derived(
    !job
      ? ''
      : job.state === 'running'
        ? `Reading ${done} of ${plural(total, 'channel')}…`
        : job.state === 'done'
          ? `Done: ${plural(total, 'channel')} read, ${plural(job.proposals, 'change')} proposed.`
          : `Cancelled after ${done} of ${plural(total, 'channel')}.`,
  );

  async function start(event: SubmitEvent) {
    event.preventDefault();
    // aria-disabled, not disabled: the key keeps focus while a job runs.
    if (running) return;
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
    else error = result.message;
    // Cancel is gone with the running state; the key takes focus back.
    await tick();
    go?.focus({ preventScroll: true });
  }
</script>

<div class="rescan">
  <form class="rescan__card" data-fid="cfg-card" onsubmit={start}>
    <div class="rescan__go">
      <div class="field">
        <span class="label">Channels</span>
        <MultiSelect size="field" label="Channels" fullLabel="Channels to re-read" bind:values={chosen} options={targets.map((t) => ({ value: t.id, label: t.name }))} noun="party channels" />
      </div>
      <div class="field">
        <span class="label">Window</span>
        <Select
          label="Window"
          bind:value={() => window_, (v) => (window_ = v as RescanJob['window'])}
          options={[
            { value: 'week', label: 'This boss week' },
            { value: 'since_reset', label: 'Since the last reset' },
            { value: 'two_weeks', label: 'The last two weeks' },
          ]}
        />
      </div>
      <button class="btn btn--primary rescan__key" type="submit" aria-disabled={running} bind:this={go}>Re-read</button>
      <span class="rescan__note">One re-read at a time.</span>
    </div>
    <p class="field__error" role="alert">{error}</p>
  </form>
  <LiveRegion message={status} />
  {#if job}
    <section class="rescan__card rescan__job rescan__job--{job.state}" data-fid="cfg-card" aria-labelledby="{uid}-job">
      <div class="rescan__jobhead">
        <h4 class="rescan__jobtitle" id="{uid}-job">
          {job.state === 'running' ? 'Re-reading' : job.state === 'done' ? 'Re-read' : 'Stopped re-reading'}
          {plural(total, 'channel')}
        </h4>
        {#if percent !== null}<span class="rescan__pct">{percent}%</span>{/if}
        {#if progress}
          <span class="rescan__status"
            >{[progress.count || (read ? `${status} · ${plural(read, 'message')} read` : status), progress.started].filter(Boolean).join(' · ')}</span
          >
        {:else}
          <span class="rescan__status">{status}{#if read}<span class="rescan__read">{` · ${plural(read, 'message')} read`}</span>{/if}</span>
        {/if}
        {#if running}<button class="btn rescan__cancel" type="button" onclick={() => void cancel()}>Cancel</button>{/if}
      </div>
      <!-- The page's one moving wave while a job reads; a finished bar settles flat. -->
      {#if progress?.count}
        <WavyProgress value={Math.min(job.messages ?? 0, job.messages_total ?? 0)} max={job.messages_total ?? 0} label="Rescan progress" text="{progress.count} read" />
      {:else}
        <WavyProgress value={done} max={total} wavy={running} label="Rescan progress" text="{done} of {total} channels read" />
      {/if}
      <p class="rescan__found">
        {job.state === 'running' ? 'Found so far:' : 'Found:'}
        <b>{plural(job.proposals, 'change')}</b>{#if job.proposals}&nbsp;(sent to the <a href="/inbox">Inbox</a>){/if}{#if details}&nbsp;· details on <a href="/extractions">Extractions</a>{/if}
      </p>
    </section>
  {/if}
</div>

<style>
  .rescan {
    display: flex;
    flex-direction: column;
    gap: 0.875rem;
  }

  /* The settings card (spec "Panel"), drawn here so Extractions gets it too. */
  .rescan__card {
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
    min-width: 0;
    margin: 0;
    padding: 1rem 1.125rem;
    border-radius: 20px;
    background: var(--row);
    box-shadow: inset 0 0 0 1.5px var(--line);
  }

  .rescan__go {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.625rem;
    margin-top: 4px;
  }

  .rescan__go .field {
    margin: 0;
  }

  .rescan__go .field {
    min-width: 12.5rem;
  }

  .rescan__key {
    min-height: 40px;
    padding: 0 1.375rem;
    border-radius: 20px;
    font-weight: 700;
  }

  .rescan__key[aria-disabled='true'] {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .rescan__note {
    align-self: center;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }

  .field__error {
    margin: 0;
  }

  .field__error:empty {
    display: none;
  }

  /* The job card: the selection fill while running, a plain card after. */
  .rescan__job {
    gap: 0.5rem;
  }

  .rescan__job--running {
    background: var(--select);
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
  }

  .rescan__jobhead {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.25rem 0.625rem;
  }

  .rescan__jobtitle {
    margin: 0;
    font-family: var(--body);
    font-size: var(--fs-body);
    font-weight: 700;
  }

  .rescan__pct {
    font-family: var(--mono);
    font-size: var(--fs-small);
  }

  .rescan__status {
    flex: 1 1 12rem;
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  .rescan__cancel {
    align-self: center;
    min-height: 32px;
    border-radius: 16px;
  }

  .rescan__found {
    margin: 0;
    font-size: var(--fs-small);
  }
</style>
