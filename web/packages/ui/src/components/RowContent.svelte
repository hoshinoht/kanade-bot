<script lang="ts">
  import type { Snippet } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { PHONE_QUERY } from '../media';

  let { expanded, compact, children }: { expanded: boolean; compact: Snippet; children: Snippet } = $props();
  // Phones keep every row on its compact line; the detail view carries the rest.
  const phone = new MediaQuery(PHONE_QUERY, false);
  const open = $derived(expanded && !phone.current);
</script>

<!-- Both faces share a track: the compact line sets the floor while the full
     face animates its intrinsic height, without measurements or scroll calls. -->
<span class="row-content" class:row-content--expanded={open}>
  <span class="row-content__compact" aria-hidden={open}>{@render compact()}</span>
  <span class="row-content__reveal" aria-hidden={!open}>
    <span class="row-content__clip"><span class="row-content__full">{@render children()}</span></span>
  </span>
</span>
