<!--
  The Ownership tab's open requests as a listbox, oldest first: the timing
  leads, then who asks and who owns it now, then when it expires. Keyboard
  as InboxList: wide (`follow`) selection follows the arrows; phones move a
  focus-only active option and Enter/Space/click opens it.
-->
<script lang="ts">
  import type { OwnershipRequest } from '@kanade/api-types';
  import { tick } from 'svelte';
  import { ListPane, Portrait } from '@kanade/ui';
  import { expiresIn, handover, ownershipTitle } from './ownership';

  let {
    items,
    selected,
    follow,
    now,
    onpick,
    fresh,
  }: {
    items: OwnershipRequest[];
    selected: string;
    follow: boolean;
    /** The server's clock (`Week.generated_at`); empty leaves the expiry out. */
    now: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
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
  label="Ownership requests"
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
  {#each items as r (r.id)}
    <li
      class="inbox__option"
      class:inbox__option--art={Boolean(r.bosses[0])}
      data-fid="ownership-row"
      class:inbox__option--active={!follow && r.id === current}
      id="{uid}-{r.id}"
      role="option"
      aria-selected={r.id === selected}
      data-item={r.id}
      data-new={fresh?.has(r.id) ? '' : undefined}
    >
      {#if r.bosses[0]}<span class="inbox__art" aria-hidden="true"><Portrait boss={r.bosses[0]} size="md" /></span>{/if}
      <span class="inbox__lines">
        <span class="inbox__what">{ownershipTitle(r)}</span>
        <span class="inbox__meta"><span class="mono">{r.weekday_name} {r.time}</span> · <span>{handover(r)}</span></span>
        {#if expiresIn(r, now)}<span class="inbox__count">{expiresIn(r, now)}</span>{/if}
      </span>
    </li>
  {/each}
</ListPane>
