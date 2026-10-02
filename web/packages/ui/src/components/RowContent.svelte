<script lang="ts">
  import type { Snippet } from 'svelte';

  let { expanded, compact, children }: { expanded: boolean; compact: Snippet; children: Snippet } = $props();
</script>

<!-- Both faces share a track: the compact line sets the floor while the full
     face animates its intrinsic height, without measurements or scroll calls. -->
<span class="row-content" class:row-content--expanded={expanded}>
  <span class="row-content__compact" aria-hidden={expanded}>{@render compact()}</span>
  <span class="row-content__reveal" aria-hidden={!expanded}>
    <span class="row-content__clip"><span class="row-content__full">{@render children()}</span></span>
  </span>
</span>
