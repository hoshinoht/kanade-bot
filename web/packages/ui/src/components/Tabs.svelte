<script lang="ts" module>
  export interface TabItem<T extends string = string> {
    id: T;
    label: string;
    count?: number | null;
  }
</script>

<script lang="ts" generics="T extends string">
  import type { Snippet } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';

  let {
    items,
    selected = $bindable(),
    label,
    panel,
    actions,
  }: {
    items: TabItem<T>[];
    selected: T;
    label: string;
    panel: Snippet<[T]>;
    actions?: Snippet;
  } = $props();

  const uid = $props.id();
  const tabs: Record<string, HTMLButtonElement> = {};
  // Panels mount on first visit and then stay, so scroll and form state survive tab switches.
  const visited = new SvelteSet<string>();
  $effect.pre(() => {
    visited.add(selected);
  });

  function select(index: number) {
    const item = items[(index + items.length) % items.length];
    if (!item) return;
    selected = item.id;
    tabs[item.id]?.focus();
  }

  function onKeydown(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: items.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    select(target);
  }
</script>

<section class="card tabs" aria-label={label}>
  <div class="card__head tabs__strip">
    <div class="tabs__tabs" role="tablist" aria-label={label}>
      {#each items as item, index (item.id)}
        <button
          type="button"
          role="tab"
          class="tabs__tab"
          id="{uid}-tab-{item.id}"
          aria-selected={selected === item.id}
          aria-controls="{uid}-panel-{item.id}"
          tabindex={selected === item.id ? 0 : -1}
          bind:this={tabs[item.id]}
          onclick={() => (selected = item.id)}
          onkeydown={(event) => onKeydown(event, index)}
        >
          {item.label}
          {#if item.count != null}<span class="tabs__count">{item.count}</span>{/if}
        </button>
      {/each}
    </div>
    {@render actions?.()}
  </div>
  {#each items as item (item.id)}
    <div
      class="tabs__panel"
      role="tabpanel"
      id="{uid}-panel-{item.id}"
      aria-labelledby="{uid}-tab-{item.id}"
      tabindex="0"
      hidden={selected !== item.id}
      data-panel={item.id}
    >
      {#if visited.has(item.id)}{@render panel(item.id)}{/if}
    </div>
  {/each}
</section>
