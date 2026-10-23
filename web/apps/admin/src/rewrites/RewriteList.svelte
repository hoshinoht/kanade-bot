<!--
  The rewrite attempts as a listbox, in the Extractions list's two-line rows:
  the time and the verdict pill, then the kind and "stage · code · latency ·
  tokens" (the facts give way first; the opened attempt shows each in full).
-->
<script lang="ts">
  import type { RewriteRow } from '@kanade/api-types';
  import { KIND_LABEL, OUTCOME_LABEL } from '../logs/filters';
  import { duration } from '../logs/format';
  import LogTime from '../logs/LogTime.svelte';
  import TokenUsage from '../logs/TokenUsage.svelte';
  import OptionList from '../shared/OptionList.svelte';
  import { verdictTone } from './format';

  let {
    rows,
    selected,
    follow,
    timeZone,
    onpick,
    fresh,
  }: {
    rows: RewriteRow[];
    selected: string;
    follow: boolean;
    timeZone: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
  } = $props();
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();

  export const focusOn = async (id: string) => list?.focusOn(id);
</script>

<OptionList
  bind:this={list}
  label="Rewrite attempts, newest first"
  items={rows}
  {selected}
  {follow}
  {onpick}
  {fresh}
  key="attempt"
  rowClass={(row) => ['extract-row', row.id === selected && 'extract-row--selected']}
  activeClass="extract-row--active"
>
  {#snippet option(row)}
    <span class="extract-row__line">
      <span class="extract-row__at mono"><LogTime at={row.at} {timeZone} /></span>
      <span class="tone tone--{verdictTone(row.verdict)} extract-row__out">{OUTCOME_LABEL[row.verdict] ?? row.verdict}</span>
    </span>
    <span class="extract-row__line extract-row__meta">
      <span class="extract-row__channel">{KIND_LABEL[row.kind] ?? row.kind}</span>
      <span class="extract-row__facts mono"
        >{row.stage} · {#if row.code ?? row.rule}{row.code ?? row.rule} ·&nbsp;{/if}{duration(row.latency_ms)}{#if row.prompt_tokens != null || row.completion_tokens != null}&nbsp;· <TokenUsage
            prompt={row.prompt_tokens}
            cached={row.cached_tokens}
            completion={row.completion_tokens}
          />{/if}</span
      >
    </span>
    {#if row.id === selected}<span class="vh">open</span>{/if}
  {/snippet}
</OptionList>
