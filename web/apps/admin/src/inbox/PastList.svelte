<!--
  The Past tab's closed items as a listbox, newest first: the outcome and
  summary lead, then the kind and source, then who closed it and when.
  Keyboard as InboxList: wide (`follow`) selection follows the arrows;
  phones move a focus-only active option and Enter/Space/click opens it.
-->
<script lang="ts">
  import type { PastItem } from '@kanade/api-types';
  import { tick } from 'svelte';
  import { ListPane } from '@kanade/ui';
  import { localAt } from '../history/describe';
  import { SOURCE_LABEL } from './flags';
  import OutcomeChip from './OutcomeChip.svelte';

  let {
    items,
    selected,
    follow,
    timeZone,
    onpick,
  }: {
    items: PastItem[];
    selected: string;
    follow: boolean;
    timeZone: string;
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

{#if items.length}
  <ListPane
    label="Past items"
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
        class="inbox__option past__option"
        data-fid="past-row"
        class:inbox__option--active={!follow && p.id === current}
        id="{uid}-{p.id}"
        role="option"
        aria-selected={p.id === selected}
        data-item={p.id}
      >
        <span class="inbox__lines">
          <span class="past__rowhead"><OutcomeChip outcome={p.outcome} /><span class="inbox__what past__summary">{p.summary}</span></span>
          <span class="inbox__meta">{p.kind_label} · {SOURCE_LABEL[p.source]}</span>
          <span class="inbox__meta">{p.decided_by ? `${p.decided_by.name} · ` : ''}<time class="mono" datetime={p.decided_at}>{localAt(p.decided_at, timeZone)}</time></span>
        </span>
      </li>
    {/each}
  </ListPane>
{:else}
  <section class="inbox__empty" aria-label="No past items">
    <h2>Nothing closed yet</h2>
    <p>No closed items yet. Approved, rejected and expired changes are kept here.</p>
  </section>
{/if}
