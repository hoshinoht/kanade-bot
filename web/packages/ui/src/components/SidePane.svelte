<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label, className = '', children }: { label: string; className?: string; children: Snippet } = $props();
</script>

<aside class={`side-pane ${className}`} aria-label={label}>
  {@render children()}
</aside>
