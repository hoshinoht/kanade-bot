<!--
  The call's outcome card beside Changes and Chat read (B_Extract aside):
  what it proposed (one card per change, with the Inbox while still
  waiting), the call's facts, then "Re-read this channel" at the foot.
-->
<script lang="ts">
  import type { Extraction } from '@kanade/api-types';
  import { OUTCOME_LABEL } from '../logs/filters';
  import { duration } from '../logs/format';
  import TokenUsage from '../logs/TokenUsage.svelte';

  let {
    call,
    canReread,
    rescanOff = null,
    onreread,
  }: { call: Extraction; canReread: boolean; rescanOff?: string | null; onreread: () => void } = $props();
  const n = $derived(call.amendments.length);
  const headline = $derived(
    n ? `${n} change${n === 1 ? '' : 's'} proposed` : call.outcome === 'proposed' || call.outcome === 'no_change' ? 'No change' : (OUTCOME_LABEL[call.outcome] ?? call.outcome),
  );
  const plural = (count: number, one: string) => `${count} ${one}${count === 1 ? '' : 's'}`;
</script>

<aside class="extract-outcome" data-fid="extract-outcome" aria-label="Outcome">
  <span class="cap extract-outcome__cap">Outcome</span>
  <p class="extract-outcome__headline">{headline}</p>
  {#if call.error}<p class="extract-outcome__error">{call.error}</p>{/if}
  {#each call.amendments as a, i (i)}
    <div class="extract-outcome__change" data-fid="extract-change">
      <b>{a.bosses} · {a.when}</b>
      <span>{a.kind} · <span class="mono">{a.confidence.toFixed(2)}</span> · {a.status}</span>
      {#if a.status === 'proposed'}<a href="/inbox?tab=extractor">Open in Inbox</a>{/if}
    </div>
  {/each}
  <span class="cap extract-outcome__call">Call</span>
  <p class="extract-outcome__facts">
    {plural(call.messages.length, 'message')} read · {plural(n, 'change')}<br />
    latency <span class="mono">{duration(call.latency_ms)}</span><br />
    tokens <span class="mono"><TokenUsage prompt={call.prompt_tokens} completion={call.completion_tokens} reasoning={call.reasoning_tokens} /></span>
    {#if call.request_ids?.length}<br />request ids <span class="mono extract-outcome__ids">{call.request_ids.join(', ')}</span>{/if}
  </p>
  <span class="extract-outcome__gap"></span>
  <!-- Only party channels can be re-read; any other says why instead of vanishing. -->
  <button
    class="btn extract-outcome__reread"
    data-fid="extract-reread-one"
    type="button"
    disabled={!canReread}
    title={canReread ? undefined : (rescanOff ?? 'Only the party channels Kanade watches can be re-read.')}
    onclick={onreread}>Re-read this channel</button
  >
</aside>

<style>
  /* Ids are long unbroken tokens; they wrap anywhere rather than widen the card. */
  .extract-outcome__ids {
    overflow-wrap: anywhere;
  }
</style>
