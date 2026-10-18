<!--
  The calls as a listbox (B_Extract list pane): two-line rows, the time and
  the outcome chip, then the channel and "n msg · latency · tokens".
-->
<script lang="ts">
  import type { ExtractionRow } from '@kanade/api-types';
  import { duration } from '../logs/format';
  import LogTime from '../logs/LogTime.svelte';
  import TokenUsage from '../logs/TokenUsage.svelte';
  import Name from '../names/Name.svelte';
  import OptionList from '../shared/OptionList.svelte';
  import { rowOutcome } from './code';

  let {
    rows,
    selected,
    follow,
    timeZone,
    onpick,
    fresh,
  }: {
    rows: ExtractionRow[];
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
  label="Extraction calls, newest first"
  items={rows}
  {selected}
  {follow}
  {onpick}
  {fresh}
  key="call"
  fid="extract-row"
  rowClass={(row) => ['extract-row', row.id === selected && 'extract-row--selected']}
  activeClass="extract-row--active"
>
  {#snippet option(row)}
    {@const out = rowOutcome(row)}
    <span class="extract-row__line">
      <span class="extract-row__at mono"><LogTime at={row.at} {timeZone} /></span>
      <span class="tone tone--{out.tone} extract-row__out">{out.text}</span>
    </span>
    <span class="extract-row__line extract-row__meta">
      <span class="extract-row__channel">{#if row.channel_id}<Name kind="channel" id={row.channel_id} name={row.channel} plain />{:else}no channel{/if}</span>
      <span class="extract-row__facts mono"
        >{row.messages} msg · {duration(row.latency_ms)}{#if row.prompt_tokens != null || row.completion_tokens != null}&nbsp;· <TokenUsage
            prompt={row.prompt_tokens}
            completion={row.completion_tokens}
            reasoning={row.reasoning_tokens}
          />{/if}</span
      >
    </span>
    {#if row.id === selected}<span class="vh">open</span>{/if}
  {/snippet}
</OptionList>
