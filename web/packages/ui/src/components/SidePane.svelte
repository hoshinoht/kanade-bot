<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label, children }: { label: string; children: Snippet } = $props();
</script>

<aside class="side-pane" aria-label={label}>
  {@render children()}
</aside>
