<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label = 'Decision', children }: { label?: string; children: Snippet } = $props();
</script>

<aside class="decision-card" aria-label={label}>
  {@render children()}
</aside>
