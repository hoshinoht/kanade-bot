<script lang="ts">
  import type { Snippet } from 'svelte';

  /** `label` names the region; `overline` is its visible mono caption. */
  let { label = 'Decision', overline = 'Decision', children }: { label?: string; overline?: string; children: Snippet } = $props();
</script>

<aside class="decision-card" aria-label={label} data-fid="decision">
  {#if overline}<p class="cap" aria-hidden="true">{overline}</p>{/if}
  {@render children()}
</aside>
