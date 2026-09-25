<!--
  The tab's items as a keyboard-navigable listbox (selection follows focus):
  boss and type lead, then who and when, then status badges in words.
-->
<script lang="ts">
  import type { Proposal } from '@kanade/api-types';
  import { FLAG_LABEL, FLAG_TONE, title, who } from './flags';

  let {
    items,
    selected,
    label,
    onpick,
  }: { items: Proposal[]; selected: string; label: string; onpick: (id: string, open: boolean) => void } = $props();
  const uid = $props.id();

  function onKeydown(event: KeyboardEvent) {
    const index = items.findIndex((p) => p.id === selected);
    const moves: Record<string, number> = {
      ArrowDown: Math.min(items.length - 1, index + 1),
      ArrowUp: Math.max(0, index - 1),
      Home: 0,
      End: items.length - 1,
    };
    if (event.key === 'Enter' && index >= 0) {
      event.preventDefault();
      onpick(items[index]!.id, true);
      return;
    }
    const target = moves[event.key];
    if (target === undefined || !items[target]) return;
    event.preventDefault();
    onpick(items[target]!.id, false);
    document.getElementById(`${uid}-${items[target]!.id}`)?.scrollIntoView({ block: 'nearest' });
  }
</script>

{#if items.length}
  <ul
    class="inbox__options"
    role="listbox"
    aria-label={label}
    tabindex="0"
    aria-activedescendant={selected ? `${uid}-${selected}` : undefined}
    onkeydown={onKeydown}
    onclick={(event) => {
      const option = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-item]') : null;
      if (option?.dataset.item) onpick(option.dataset.item, true);
    }}
  >
    {#each items as p (p.id)}
      <li
        class="inbox__option"
        id="{uid}-{p.id}"
        role="option"
        aria-selected={p.id === selected}
        data-item={p.id}
      >
        <span class="inbox__what">{title(p)}</span>
        <span class="inbox__meta"><span>{who(p)}</span> · <span class="mono">{p.when}</span></span>
        {#if p.flags.length || p.is_question}
          <span class="inbox__badges">
            {#each p.flags as flag (flag)}<span class="chip {FLAG_TONE[flag]}">{FLAG_LABEL[flag]}</span>{/each}
            {#if p.is_question}<span class="chip chip--waiting">still a question</span>{/if}
          </span>
        {/if}
      </li>
    {/each}
  </ul>
{:else}
  <p class="note inbox__none">Nothing waiting here.</p>
{/if}
