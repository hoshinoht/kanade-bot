<!--
  The interactions as a keyboard-navigable listbox (B_Chat `.crow`): the
  question and how long it took, then who · when · model and the outcome.
  Wide (`follow`): selection follows the arrows. Narrow: opening a turn hides
  the list, so arrows move a focus-only active option and Enter/Space/click
  opens it.
-->
<script lang="ts">
  import type { ChatRow } from '@kanade/api-types';
  import { ListPane, StatusChip } from '@kanade/ui';
  import { tick } from 'svelte';
  import LogTime from '../logs/LogTime.svelte';
  import { duration, preview } from '../logs/format';
  import { OUTCOME_LABEL, outcomeTone } from '../logs/filters';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import { chipTone } from './facts';
  import { mentionsText } from './transcript';

  let {
    rows,
    selected,
    follow,
    timeZone,
    onpick,
  }: {
    rows: ChatRow[];
    selected: string;
    follow: boolean;
    timeZone: string;
    onpick: (id: string, open: boolean) => void;
  } = $props();
  const uid = $props.id();
  let active = $state('');
  let listEl = $state<HTMLUListElement>();
  const current = $derived(follow ? selected : rows.some((r) => r.id === active) ? active : '');
  // Distinct aliases in round order: one model for most turns.
  const models = (row: ChatRow) => (row.models.length ? [...new Set(row.models)].join(', ') : row.model);

  const reveal = (id: string) => document.getElementById(`${uid}-${id}`)?.scrollIntoView({ block: 'nearest' });

  /** Focus the listbox with `id` (when still listed) as its active option. */
  export async function focusOn(id: string) {
    active = id;
    await tick();
    listEl?.focus({ preventScroll: true });
    if (current) reveal(current);
  }

  function onKeydown(event: KeyboardEvent) {
    const index = rows.findIndex((r) => r.id === current);
    const moves: Record<string, number> = {
      ArrowDown: index < 0 ? 0 : Math.min(rows.length - 1, index + 1),
      ArrowUp: Math.max(0, index - 1),
      Home: 0,
      End: rows.length - 1,
    };
    if ((event.key === 'Enter' || event.key === ' ') && index >= 0) {
      event.preventDefault();
      onpick(rows[index]!.id, true);
      return;
    }
    const target = moves[event.key];
    if (target === undefined || !rows[target]) return;
    event.preventDefault();
    const id = rows[target]!.id;
    if (follow) onpick(id, false);
    else active = id;
    reveal(id);
  }
</script>

<ListPane
  label="Chatbot interactions, newest first"
  activeDescendant={current ? `${uid}-${current}` : undefined}
  bind:element={listEl}
  onkeydown={onKeydown}
  onclick={(event) => {
    const option = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-item]') : null;
    if (!option?.dataset.item) return;
    active = option.dataset.item;
    onpick(option.dataset.item, true);
  }}
>
  {#each rows as row (row.id)}
    {@const took = duration(row.latency_ms)}
    <li
      class="chat-row"
      class:chat-row--active={!follow && row.id === current}
      id="{uid}-{row.id}"
      role="option"
      aria-selected={row.id === selected}
      data-item={row.id}
      data-fid="chat-row"
    >
      <span class="chat-row__top">
        <span class="chat-row__q" title={mentionsText(row.asked)}><Mentions text={preview(row.asked)} plain asked dropBot /></span>
        <span class="chat-row__took mono">{took}</span>
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
    </li>
  {/each}
</ListPane>
