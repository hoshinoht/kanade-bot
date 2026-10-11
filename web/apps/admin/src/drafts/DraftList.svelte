<!--
  The tab's drafts as a keyboard listbox (B_Drafts `drafts-list`), as the
  Inbox list: the boss portrait of the first staged run, the title, how many
  changes and on which History change, then Up to date / Base moved (or how
  it closed) and when it was last edited. Wide (`follow`): selection follows
  the arrows. Phones: arrows move a focus-only active row; Enter opens it.
-->
<script lang="ts">
  import type { Boss, DraftRow } from '@kanade/api-types';
  import { tick } from 'svelte';
  import { ListPane, Portrait, StatusChip } from '@kanade/ui';
  import { closedLabel, editedAt } from './drafts';

  let {
    items,
    selected,
    label,
    follow,
    portraits,
    timeZone,
    now = '',
    onpick,
    fresh,
  }: {
    items: DraftRow[];
    selected: string;
    label: string;
    follow: boolean;
    /** The first staged run's boss per draft, once read (absent: none yet). */
    portraits: Record<string, Boss | null>;
    timeZone: string;
    /** The server's clock (`Week.generated_at`): today's edits show the time only. */
    now?: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
  } = $props();
  const uid = $props.id();
  let active = $state('');
  let listEl = $state<HTMLUListElement>();
  const current = $derived(follow ? selected : items.some((d) => d.id === active) ? active : '');

  const reveal = (id: string) => document.getElementById(`${uid}-${id}`)?.scrollIntoView({ block: 'nearest' });

  /** Focus the listbox with `id` (when still listed) as its active option. */
  export async function focusOn(id: string) {
    active = id;
    await tick();
    listEl?.focus({ preventScroll: true });
    if (current) reveal(current);
  }

  function onKeydown(event: KeyboardEvent) {
    const index = items.findIndex((d) => d.id === current);
    const moves: Record<string, number> = {
      ArrowDown: index < 0 ? 0 : Math.min(items.length - 1, index + 1),
      ArrowUp: Math.max(0, index - 1),
      Home: 0,
      End: items.length - 1,
    };
    if ((event.key === 'Enter' || event.key === ' ') && index >= 0) {
      event.preventDefault();
      onpick(items[index]!.id, true);
      return;
    }
    const target = moves[event.key];
    if (target === undefined || !items[target]) return;
    event.preventDefault();
    const id = items[target]!.id;
    if (follow) onpick(id, false);
    else active = id;
    reveal(id);
  }
</script>

<ListPane
  {label}
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
  {#each items as d (d.id)}
    {@const boss = portraits[d.id]}
    <li
      class="inbox__option inbox__option--art drafts__row"
      data-fid="drafts-row"
      class:inbox__option--active={!follow && d.id === current}
      id="{uid}-{d.id}"
      role="option"
      aria-selected={d.id === selected}
      data-item={d.id}
      data-new={fresh?.has(d.id) ? '' : undefined}
    >
      <span class="inbox__art drafts__art" aria-hidden="true">{#if boss}<Portrait {boss} size="md" />{/if}</span>
      <span class="inbox__lines">
        <span class="inbox__what">{d.title}</span>
        <span class="inbox__meta"><b class="mono drafts__count">{d.staged}</b> staged · on <span class="mono">#{d.base}</span></span>
        <span class="inbox__badges drafts__state">
          {#if d.status !== 'open'}
            <StatusChip tone={d.status === 'merged' ? 'ok' : 'neutral'}>{closedLabel(d)}</StatusChip>
          {:else if d.up_to_date}
            <StatusChip tone="ok">Up to date</StatusChip>
          {:else}
            <StatusChip tone="warn">Base moved</StatusChip>
          {/if}
          <span class="mono drafts__edited">edited {editedAt(d.updated_at, timeZone, now)}</span>
        </span>
      </span>
    </li>
  {/each}
</ListPane>
