<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    label,
    activeDescendant = undefined,
    element = $bindable(),
    onkeydown,
    onclick,
    children,
  }: {
    label: string;
    activeDescendant?: string;
    element?: HTMLUListElement;
    onkeydown?: (event: KeyboardEvent) => void;
    onclick?: (event: MouseEvent) => void;
    children: Snippet;
  } = $props();
</script>

<aside class="list-pane" aria-label={label}>
  <ul
    class="list-pane__options"
    role="listbox"
    aria-label={label}
    tabindex="0"
    aria-activedescendant={activeDescendant}
    bind:this={element}
    {onkeydown}
    {onclick}
  >
    {@render children()}
  </ul>
</aside>
