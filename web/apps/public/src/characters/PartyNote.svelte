<!--
  The party's estimate label, planning-only note and NEXON attribution, once
  above the list (pane and phone alike). The label only while estimates are
  on; the attribution only while the member's own row carries NEXON values.
-->
<script lang="ts">
  import type { RunPartyCharacters } from '@kanade/api-types';

  let { party }: { party: RunPartyCharacters } = $props();
</script>

{#if party.estimates || party.nexon_data}
  <p class="party-chars__note" data-fid="party-chars-note">
    {#if party.estimates}<span><b>{party.estimate_label}</b> · {party.estimate_note}.</span>{/if}
    {#if party.nexon_data}<span data-fid="party-chars-attrib">{party.attribution}</span>{/if}
  </p>
{/if}
