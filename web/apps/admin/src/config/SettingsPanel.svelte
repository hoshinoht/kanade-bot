<!--
  One Config section (spec "Panel"): the heading and its one-line lead, an
  optional row of pill tabs, the settings cards, and the section's save bar.
  Only the cards scroll; the save bar stays put under them.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let {
    title,
    lead,
    tabs,
    children,
    bar,
  }: { title: string; lead?: Snippet; tabs?: Snippet; children: Snippet; bar?: Snippet } = $props();
</script>

<div class="settings__scroll" data-fid="cfg-panel">
  <div class="settings__headline" data-fid="cfg-head">
    <h3 class="settings__title" data-fid="cfg-title">{title}</h3>
    {#if lead}<p class="settings__lead">{@render lead()}</p>{/if}
  </div>
  {@render tabs?.()}
  {@render children()}
</div>
{@render bar?.()}
