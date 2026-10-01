<script lang="ts">
  import type { Snippet } from 'svelte';

  let { label = 'Thread', children }: { label?: string; children: Snippet } = $props();
</script>

<section class="thread-panel" aria-label={label}>
  {@render children()}
</section>
