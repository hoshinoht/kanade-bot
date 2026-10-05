<!--
  The calls as a listbox (B_Extract list pane): two-line rows, the time and
  the outcome chip, then the channel and "n msg · latency · tokens". Wide:
  the selection follows the arrows. Phones: the list stands alone, so arrows
  move a focus-only active option and Enter/Space/click opens it.
-->
<script lang="ts">
  import type { ExtractionRow } from '@kanade/api-types';
  import { ListPane } from '@kanade/ui';
  import { tick } from 'svelte';
  import { duration } from '../logs/format';
  import LogTime from '../logs/LogTime.svelte';
  import TokenUsage from '../logs/TokenUsage.svelte';
  import Name from '../names/Name.svelte';
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
    /** Calls a live update just brought in: marked once (`data-new`). */
    fresh?: ReadonlySet<string>;
  } = $props();
  const uid = $props.id();
  let active = $state('');
  let listEl = $state<HTMLUListElement>();
  const current = $derived(follow ? selected : rows.some((r) => r.id === active) ? active : '');

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
  label="Extraction calls, newest first"
  activeDescendant={current ? `${uid}-${current}` : undefined}
  bind:element={listEl}
  onkeydown={onKeydown}
  onclick={(event) => {
    const option = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-call]') : null;
    if (!option?.dataset.call) return;
    active = option.dataset.call;
    onpick(option.dataset.call, true);
  }}
>
  {#each rows as row (row.id)}
    {@const out = rowOutcome(row)}
    <li
      class="extract-row"
      class:extract-row--selected={row.id === selected}
      class:extract-row--active={!follow && row.id === current}
      data-fid="extract-row"
      id="{uid}-{row.id}"
      data-new={fresh?.has(row.id) ? '' : undefined}
      role="option"
      aria-selected={row.id === selected}
      data-call={row.id}
    >
      <span class="extract-row__line">
        <span class="extract-row__at mono"><LogTime at={row.at} {timeZone} /></span>
        <span class="tone tone--{out.tone} extract-row__out">{out.text}</span>
      </span>
      <span class="extract-row__line extract-row__meta">
        <span class="extract-row__channel">{#if row.channel_id}<Name kind="channel" id={row.channel_id} name={row.channel} plain />{:else}no channel{/if}</span>
        <span class="extract-row__facts mono"
          >{row.messages} msg · {duration(row.latency_ms)}{#if row.prompt_tokens != null || row.completion_tokens != null}
            · <TokenUsage prompt={row.prompt_tokens} completion={row.completion_tokens} reasoning={row.reasoning_tokens} />{/if}</span
        >
      </span>
      {#if row.id === selected}<span class="vh">open</span>{/if}
    </li>
  {/each}
</ListPane>
