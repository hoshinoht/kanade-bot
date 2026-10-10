<!--
  The signed-out window on a phone (boards PhoneGate, PhoneClosed,
  PhoneDenied, PhoneEnded): one window under the top bar, its title bar,
  then the screen's own content (Sign in's banner and body, or a note).
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let { bar, fill = true, children }: { bar: string; /** Fills the frame (a note); Sign in keeps its own height. */ fill?: boolean; children: Snippet } = $props();
</script>

<section class="card gate-card" class:window-fill={fill} aria-label={bar} data-fid="gate">
  <div class="card__head" data-fid="gate-bar"><span class="card__title">{bar}</span></div>
  {@render children()}
</section>
