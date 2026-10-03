<!--
  The loading standard (m3e-rail-design-spec "Motion and loading"): nothing
  for the first 200 ms, then the indicator centred in the waiting pane with
  visible words. With design experiments off the words show alone (the
  morphing indicator is Experiment A); reduced motion stills the shape.
-->
<script lang="ts">
  import { experiments } from '../experiments/experiments.svelte';
  import { Delay } from '../motion/delay.svelte';

  let { text }: { text: string } = $props();

  const delay = new Delay();
  $effect(() => {
    delay.start();
    return () => delay.stop();
  });
</script>

<!-- The status region exists from the start (empty), so screen readers have
  registered it before the words arrive; only its content waits 200 ms. -->
<div class="loading-state">
  <p class="loading-state__body" role="status">
    {#if delay.due}
      {#if experiments.on}<span class="xp-loading" aria-hidden="true"><span class="xp-loading__shape"></span></span>{/if}<span>{text}</span>
    {/if}
  </p>
</div>
