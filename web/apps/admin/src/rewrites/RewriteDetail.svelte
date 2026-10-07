<!--
  One rewrite attempt in the Rewrites detail pane, in the Extractions call's
  layout: an eyebrow with the id, the time as the heading, model · kind ·
  stage · latency; the seed, the model's reply, the line used and its
  reasoning on the pane, beside the verdict card (rule or error code, usage
  against the reservation, context, request id).
-->
<script lang="ts">
  import type { Rewrite } from '@kanade/api-types';
  import { LoadingState } from '@kanade/ui';
  import { KIND_LABEL, STAGE_LABEL } from '../logs/filters';
  import { duration } from '../logs/format';
  import LogTime from '../logs/LogTime.svelte';
  import Reasoning from '../logs/Reasoning.svelte';
  import TokenUsage from '../logs/TokenUsage.svelte';
  import { Resource } from '../resource.svelte';
  import { budget, overran, verdictText } from './format';

  let { id, timeZone }: { id: string; timeZone: string } = $props();
  const uid = $props.id();
  const attempt = $derived(new Resource<Rewrite>(`/api/admin/rewrites/${encodeURIComponent(id)}`));
  $effect(() => void attempt.load());

  let root = $state<HTMLElement>();
  /** Phones: an opened attempt takes focus, as a page would. */
  export function focus() {
    root?.focus({ preventScroll: true });
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article class="extract-detail" aria-labelledby="{uid}-title" tabindex="-1" bind:this={root}>
  {#if attempt.error}
    <div class="empty" role="alert"><strong>No rewrite “{id}”.</strong>{attempt.error}</div>
  {:else if attempt.data}
    {@const data = attempt.data}
    {@const spent = budget(data)}
    <header class="extract-detail__head">
      <div class="extract-detail__title">
        <span class="cap">Rewrite · #{data.short_id}</span>
        <h2 class="extract-detail__when" id="{uid}-title"><LogTime at={data.at} {timeZone} /></h2>
        <p class="extract-detail__meta">
          <span class="mono">{data.model ?? 'no model call'}</span>{#if data.reasoning} · <span class="mono">{data.reasoning}</span>{/if} ·
          {KIND_LABEL[data.kind] ?? data.kind} · {STAGE_LABEL[data.stage] ?? data.stage} ·
          <span class="mono">{data.latency_ms !== null ? `${data.latency_ms.toLocaleString('en')} ms` : 'not called'}</span>
        </p>
      </div>
    </header>
    <div class="extract-detail__body">
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <section class="extract-changes rewrite-text" aria-label="Seed, reply and line" tabindex="0">
        <dl>
          <dt class="cap">Seed</dt>
          <dd class="mono">{data.seed}</dd>
          <dt class="cap">Reply</dt>
          <dd>{#if data.reply}<pre class="rewrite-text__reply">{data.reply}</pre>{:else}<span class="note">No reply (nothing came back).</span>{/if}</dd>
          <dt class="cap">Line used</dt>
          <dd class="mono">{data.line ?? '—'}</dd>
        </dl>
        <Reasoning text={data.reasoning_content} tokens={data.reasoning_tokens} />
      </section>
      <aside class="extract-outcome" aria-label="Verdict">
        <span class="cap extract-outcome__cap">Verdict</span>
        <p class="extract-outcome__headline">{verdictText(data)}</p>
        {#if spent}<p class:extract-outcome__error={overran(data)} class="rewrite-budget">{spent}</p>{/if}
        <span class="cap extract-outcome__call">Call</span>
        <p class="extract-outcome__facts">
          latency <span class="mono">{duration(data.latency_ms)}</span><br />
          tokens <span class="mono"><TokenUsage prompt={data.prompt_tokens} completion={data.completion_tokens} reasoning={data.reasoning_tokens} /></span><br />
          max tokens <span class="mono">{data.max_output_tokens ?? '—'}</span> · prompt estimate <span class="mono">{data.prompt_estimate ?? '—'}</span>
          {#if data.context}<br />for <span class="mono rewrite-wrap">{data.context}</span>{/if}
          {#if data.request_id}<br />request id <span class="mono rewrite-wrap">{data.request_id}</span>{/if}
        </p>
      </aside>
    </div>
  {:else}
    <LoadingState text="Loading the rewrite…" />
  {/if}
</article>

<style>
  .rewrite-text dl {
    display: grid;
    gap: 4px 0;
    margin: 0;
  }

  .rewrite-text dd {
    margin: 0 0 10px;
    overflow-wrap: anywhere;
  }

  .rewrite-text__reply {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: var(--mono);
  }

  .rewrite-budget {
    margin: 0;
    font-size: var(--fs-small);
  }

  /* Keys and ids are long unbroken tokens; they wrap anywhere rather than widen the card. */
  .rewrite-wrap {
    overflow-wrap: anywhere;
  }
</style>
