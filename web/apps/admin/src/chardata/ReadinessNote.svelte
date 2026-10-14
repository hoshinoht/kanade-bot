<!--
  The line over a readiness list (module `character_data`): "Readiness" and
  what it is, the NEXON attribution while a NEXON value shows, and, once any
  scouter reading shows, its label and note (once per list). The run
  sheet's Party tab (`party-ready-*`) and the Fixed editor's Characters tab
  (`fixed-ready-*`) draw it; `fid` is the tags' stem.
-->
<script lang="ts">
  import '@kanade/ui/styles/screens/character-data.scss';
  import { ESTIMATE_LABEL, ESTIMATE_NOTE } from '@kanade/ui';

  let {
    tag = 'p',
    lead,
    attribution,
    scouter,
    replaces = false,
    fid,
  }: {
    tag?: 'p' | 'li';
    /** After "Readiness": " · never blocks". */
    lead: string;
    /** `Data based on NEXON Open API`, or null while no NEXON value shows. */
    attribution: string | null;
    /** Some row shows a scouter reading. */
    scouter: boolean;
    /** Say that a reading replaces CP. */
    replaces?: boolean;
    fid: string;
  } = $props();
  // Composed here, as Modal's part tags: the build strips only literal or plain-expression `data-fid`s.
  const fids = $derived({ note: `${fid}-note`, attrib: `${fid}-attrib`, estimate: `${fid}-estimate` });
</script>

<svelte:element this={tag} class="ready__note" data-fid={fids.note}
  ><span><b>Readiness</b>{lead}</span>{#if attribution}<span class="chardata__attrib" data-fid={fids.attrib}>{attribution}</span>{/if}</svelte:element
>
{#if scouter}
  <svelte:element this={tag} class="ready__estimate" data-fid={fids.estimate}
    ><b>{ESTIMATE_LABEL}</b> · {ESTIMATE_NOTE}.{replaces ? ' It replaces CP where a character has one.' : ''}</svelte:element
  >
{/if}
