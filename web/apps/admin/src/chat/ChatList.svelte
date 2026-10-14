<!--
  The interactions as a keyboard-navigable listbox (B_Chat `.crow`): the
  question and how long it took, then who · when · model and the outcome.
-->
<script lang="ts">
  import type { ChatRow } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import LogTime from '../logs/LogTime.svelte';
  import { duration, preview } from '../logs/format';
  import { OUTCOME_LABEL, outcomeTone } from '../logs/filters';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import OptionList from '../shared/OptionList.svelte';
  import { chipTone } from './facts';
  import { mentionsText } from '../logs/transcript';

  let {
    rows,
    selected,
    follow,
    timeZone,
    onpick,
    fresh,
  }: {
    rows: ChatRow[];
    selected: string;
    follow: boolean;
    timeZone: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
  } = $props();
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();
  // Distinct aliases in round order: one model for most turns.
  const models = (row: ChatRow) => (row.models.length ? [...new Set(row.models)].join(', ') : row.model);

  export const focusOn = async (id: string) => list?.focusOn(id);
</script>

<OptionList
  bind:this={list}
  label="Chatbot interactions, newest first"
  items={rows}
  {selected}
  {follow}
  {onpick}
  {fresh}
  fid="chat-row"
  rowClass={() => 'chat-row'}
  activeClass="chat-row--active"
>
  {#snippet option(row)}
    <span class="chat-row__top">
      <span class="chat-row__q" title={mentionsText(row.asked)}><Mentions text={preview(row.asked)} plain asked dropBot /></span>
      <span class="chat-row__took mono">{duration(row.latency_ms)}</span>
    </span>
    <span class="chat-row__meta">
      <span class="chat-row__who"><Name kind="member" id={row.member_id || row.member.id} name={row.member.name} plain /></span>
      <span aria-hidden="true">·</span>
      <span class="mono"><LogTime at={row.at} {timeZone} /></span>
      <span class="chat-row__end">
        <span class="chat-row__model mono" title={models(row)}>{models(row)}</span>
        <span class="chat-row__outcome"><StatusChip tone={chipTone(row.outcome)} legacyTone={outcomeTone(row.outcome)}>{OUTCOME_LABEL[row.outcome] ?? row.outcome}</StatusChip></span>
      </span>
    </span>
  {/snippet}
</OptionList>
