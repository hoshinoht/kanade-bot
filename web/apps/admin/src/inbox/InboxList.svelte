<!--
  The tab's items as a keyboard-navigable listbox: boss and type lead, then
  who and when, then status badges in words. Wide (`follow`): selection
  follows the arrows. Phones: opening an item hides the list, so arrows move
  a focus-only active option and Enter/Space/click opens it.
-->
<script lang="ts">
  import type { Proposal } from '@kanade/api-types';
  import { tick } from 'svelte';
  import { ListPane, Portrait, RowContent, StatusChip } from '@kanade/ui';
  import { FLAG_LABEL, FLAG_TONE, title, who } from './flags';

  let {
    items,
    selected,
    label,
    follow,
    onpick,
  }: {
    items: Proposal[];
    selected: string;
    label: string;
    follow: boolean;
    onpick: (id: string, open: boolean) => void;
  } = $props();
  const uid = $props.id();
  let active = $state('');
  let listEl = $state<HTMLUListElement>();
  const current = $derived(follow ? selected : items.some((p) => p.id === active) ? active : '');

  const reveal = (id: string) => document.getElementById(`${uid}-${id}`)?.scrollIntoView({ block: 'nearest' });

  /** Focus the listbox with `id` (when still listed) as its active option. */
  export async function focusOn(id: string) {
    active = id;
    await tick();
    listEl?.focus({ preventScroll: true });
    if (current) reveal(current);
  }

  function onKeydown(event: KeyboardEvent) {
    const index = items.findIndex((p) => p.id === current);
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
  {#each items as p (p.id)}
    <li
      class="inbox__option expandable-row"
      class:inbox__option--art={p.tab === 'extractor' && Boolean(p.bosses[0])}
      data-fid="inbox-row"
      class:inbox__option--active={!follow && p.id === current}
      id="{uid}-{p.id}"
      role="option"
      aria-selected={p.id === selected}
      data-item={p.id}
    >
      {#if p.tab === 'extractor' && p.bosses[0]}
        <!-- Extractor rows (VarRail2): the boss art, then the lines; how much of the thread was used. -->
        <span class="inbox__art" aria-hidden="true"><Portrait boss={p.bosses[0]} size="md" /></span>
      {/if}
      <RowContent expanded={p.id === selected}>
        {#snippet compact()}<span class="inbox__what">{title(p)}</span> <span class="inbox__meta">· {who(p)} · {p.when}</span>{#if p.thread?.length}<span class="inbox__count"> · {p.thread.length} messages · {p.thread.filter((m) => m.used).length} used</span>{/if}{#each p.flags as flag (flag)} · {FLAG_LABEL[flag]}{/each}{#if p.is_question} · still a question{/if}{/snippet}
        <span class="inbox__lines">
        <span class="inbox__what">{title(p)}</span>
        <span class="inbox__meta"><span>{who(p)}</span> · <span class="mono">{p.when}</span></span>
        {#if p.tab === 'extractor' && p.thread?.length}
          <span class="inbox__count">{p.thread.length} message{p.thread.length === 1 ? '' : 's'} · {p.thread.filter((m) => m.used).length} used</span>
        {/if}
        {#if p.flags.length || p.is_question}
          <span class="inbox__badges">
            {#each p.flags as flag (flag)}<StatusChip tone={FLAG_TONE[flag] === 'danger' ? 'risk' : 'warn'} legacyTone={FLAG_TONE[flag]}>{FLAG_LABEL[flag]}</StatusChip>{/each}
            {#if p.is_question}<StatusChip tone="warn">still a question</StatusChip>{/if}
          </span>
        {/if}
        </span>
      </RowContent>
    </li>
  {/each}
</ListPane>
