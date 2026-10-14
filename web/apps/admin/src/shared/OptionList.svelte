<!--
  A list pane's items as a keyboard-navigable listbox. Wide (`follow`): the
  selection follows the arrows. Narrow: opening an item hides the list, so
  arrows move a focus-only active option and Enter/Space/click opens it.
-->
<script lang="ts" generics="T extends { id: string }">
  import { ListPane } from '@kanade/ui';
  import { tick, type Snippet } from 'svelte';
  import type { ClassValue } from 'svelte/elements';

  let {
    label,
    items,
    selected,
    follow,
    onpick,
    fresh,
    key = 'item',
    fid,
    rowClass,
    activeClass,
    option,
  }: {
    label: string;
    items: T[];
    selected: string;
    follow: boolean;
    onpick: (id: string, open: boolean) => void;
    /** Items a live update just brought in: marked once (`data-new`). */
    fresh?: ReadonlySet<string>;
    /** The option's id attribute, `data-<key>`. */
    key?: string;
    fid?: string;
    rowClass: (item: T) => ClassValue;
    activeClass: string;
    option: Snippet<[T]>;
  } = $props();
  const uid = $props.id();
  let active = $state('');
  let listEl = $state<HTMLUListElement>();
  const current = $derived(follow ? selected : items.some((r) => r.id === active) ? active : '');

  const reveal = (id: string) => document.getElementById(`${uid}-${id}`)?.scrollIntoView({ block: 'nearest' });

  /** Focus the listbox with `id` (when still listed) as its active option. */
  export async function focusOn(id: string) {
    active = id;
    await tick();
    listEl?.focus({ preventScroll: true });
    if (current) reveal(current);
  }

  function onKeydown(event: KeyboardEvent) {
    const index = items.findIndex((r) => r.id === current);
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
    const id = event.target instanceof Element ? event.target.closest<HTMLElement>(`[data-${key}]`)?.dataset[key] : undefined;
    if (!id) return;
    active = id;
    onpick(id, true);
  }}
>
  {#each items as item (item.id)}
    <li
      class={[rowClass(item), !follow && item.id === current && activeClass]}
      data-fid={fid}
      id="{uid}-{item.id}"
      data-new={fresh?.has(item.id) ? '' : undefined}
      role="option"
      aria-selected={item.id === selected}
      {...{ [`data-${key}`]: item.id }}
    >
      {@render option(item)}
    </li>
  {/each}
</ListPane>
