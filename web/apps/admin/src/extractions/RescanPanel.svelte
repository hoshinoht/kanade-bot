<!--
  Re-read the party channels (v4 rescan_job.html; M3E B_CfgReread). Channel
  checkbox chips and the window choice in one card; a running job gets its
  own card with progress, what it found so far and Cancel. The job is polled
  with the bounded poller instead of v4's self-replacing htmx fragment, and
  its progress line is repeated in a polite live region.
-->
<script lang="ts">
  import type { Channel, RescanJob } from '@kanade/api-types';
  import { createClient, createPoller } from '@kanade/client';
  import { experiments, LiveRegion, WavyProgress } from '@kanade/ui';
  import { tick } from 'svelte';
  import { send } from '../resource.svelte';

  let { targets, details = false }: { targets: Channel[]; /** Link the job card to Extractions (Config). */ details?: boolean } = $props();

  const uid = $props.id();
  let chosen = $state<string[]>([]);
  let window_ = $state<RescanJob['window']>('week');
  let job = $state<RescanJob | null>(null);
  let error = $state('');
  let go: HTMLButtonElement | undefined = $state();
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

  const running = $derived(job?.state === 'running');
  const done = $derived(job ? job.channels.filter((c) => c.state === 'done').length : 0);
  const total = $derived(job?.channels.length ?? 0);
  const percent = $derived(total ? Math.round((done / total) * 100) : 0);
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
    <fieldset class="rescan__channels">
      <legend class="label">Channels · {chosen.length} of {targets.length}</legend>
      <div class="rescan__picks">
        <button class="rescan__link" type="button" disabled={targets.length === 0 || chosen.length === targets.length} onclick={() => (chosen = targets.map((t) => t.id))}
          >Select all party channels</button
        >
        <button class="rescan__link" type="button" disabled={chosen.length === 0} onclick={() => (chosen = [])}>Clear</button>
      </div>
      <div class="rescan__chips">
        {#each targets as t (t.id)}
          <label class="rescan__chk"
            ><input type="checkbox" value={t.id} bind:group={chosen} /><span class="rescan__box" aria-hidden="true">✓</span>{t.name}</label
          >
        {/each}
      </div>
    </fieldset>
    <div class="rescan__go">
      <label class="field">
        <span class="label">Window</span>
        <select bind:value={window_}>
          <option value="week">This boss week</option>
          <option value="since_reset">Since the last reset</option>
          <option value="two_weeks">The last two weeks</option>
        </select>
      </label>
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
        <span class="rescan__pct">{percent}%</span>
        <span class="rescan__status">{status}{#if read}<span class="rescan__read">{` · ${plural(read, 'message')} read`}</span>{/if}</span>
        {#if running}<button class="btn rescan__cancel" type="button" onclick={() => void cancel()}>Cancel</button>{/if}
      </div>
      {#if experiments.on}
        <WavyProgress value={done} max={total} label="Rescan progress" text="{done} of {total} channels read" />
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

  /* Legend and the select-all/clear pair on one line, then the chips. */
  .rescan__channels {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.625rem;
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }

  .rescan__channels legend {
    float: left;
    padding: 0;
    white-space: nowrap;
  }

  /* Beside the legend while they fit, else on their own line. */
  .rescan__picks {
    display: flex;
    gap: 0.75rem;
    margin-left: auto;
  }

  .rescan__link {
    padding: 0.25rem 0;
    border: 0;
    background: none;
    color: var(--accent-text);
    font: inherit;
    font-size: var(--fs-small);
    text-decoration: underline;
    text-decoration-color: color-mix(in srgb, var(--accent) 35%, transparent);
    text-underline-offset: 2px;
    cursor: pointer;
  }

  .rescan__link:disabled {
    color: var(--dim-text);
    text-decoration: none;
    cursor: default;
  }

  .rescan__chips {
    flex: 1 0 100%;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  /* Channel checkbox chips (the board's .chk): a square box, rounder when on. */
  .rescan__chk {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-height: 32px;
    padding: 0 12px 0 10px;
    border-radius: 8px;
    background: var(--row);
    box-shadow: inset 0 0 0 1.5px var(--line);
    font-size: var(--fs-small);
    cursor: pointer;
  }

  .rescan__chk:hover {
    background: var(--row-hover);
  }

  /* The native box covers the chip unseen, so the whole chip is its hit area. */
  .rescan__chk input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    opacity: 0;
    cursor: pointer;
  }

  .rescan__box {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    box-shadow: inset 0 0 0 1.5px var(--dim-text);
    color: transparent;
    font-size: 11px;
  }

  .rescan__chk:has(input:checked) {
    border-radius: 16px;
    background: var(--select);
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
    color: var(--select-ink);
    font-weight: 600;
  }

  .rescan__chk:has(input:checked) .rescan__box {
    background: var(--accent-fill);
    box-shadow: none;
    color: var(--accent-ink);
  }

  .rescan__chk:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
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

  .rescan__go select {
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
