<!--
  A button label that yields to the LoadingIndicator while its action is
  pending. With experiments off it renders the label alone, exactly as before.
  The hidden label keeps the button's width; the indicator's label becomes the
  button's name ("Saving…") while pending.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { experiments } from '../experiments/experiments.svelte';
  import LoadingIndicator from './LoadingIndicator.svelte';

  let { pending, label, children }: { pending: boolean; label: string; children: Snippet } = $props();
</script>

{#if experiments.on && pending}
  <span class="xp-pending"
    ><span class="xp-pending__label" aria-hidden="true">{@render children()}</span><LoadingIndicator size="sm" {label} /></span
  >
{:else}
  {@render children()}
{/if}
