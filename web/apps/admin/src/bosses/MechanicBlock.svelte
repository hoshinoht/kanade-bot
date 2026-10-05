<!--
  A reusable mechanic diagram: a ledger of gains and losses, named zones, or
  a range scale. Tones are a second cue; every label is printed.
-->
<script lang="ts">
  import type { Mechanic } from '@kanade/api-types';

  import { figureParts } from './guide';

  let { mechanic }: { mechanic: Mechanic } = $props();

  // Band widths through CSSOM (no inline styles under the CSP).
  const span = (value: number) => (node: HTMLElement) => {
    node.style.setProperty('flex-grow', String(value));
  };
</script>

<section class="guide-card guide-mechanic guide-mechanic--{mechanic.kind}">
  <h3>{mechanic.title}</h3>
  {#if mechanic.kind === 'ledger'}
    <ul class="guide-ledger">
      {#each mechanic.rows as row, index (index)}
        <li>
          <span>{row.label}</span>
          <span class="mono guide-ledger__value" class:guide-up={row.direction === 'up'} class:guide-down={row.direction === 'down'}>
            {#if row.direction}<span aria-hidden="true">{row.direction === 'up' ? '▲' : '▼'}</span>{/if}{row.value}{#if row.direction}<span class="vh">{row.direction === 'up' ? ', in your favour' : ', against you'}</span>{/if}
          </span>
        </li>
      {/each}
    </ul>
  {:else if mechanic.kind === 'zones'}
    <ul class="guide-zones guide-zones--n{mechanic.zones.length}">
      {#each mechanic.zones as zone, index (index)}
        <li class="guide-tone--{zone.tone ?? 'neutral'}"><strong>{zone.name}</strong>{#if zone.sub}<span>{zone.sub}</span>{/if}</li>
      {/each}
    </ul>
  {:else}
    <ol class="guide-scale">
      {#each mechanic.bands as band, index (index)}
        <li class="guide-tone--{band.tone ?? 'neutral'}" {@attach span(band.span)}><span>{#each figureParts(band.label) as part, at (at)}{#if part.figure}<span class="guide-figure">{part.text}</span>{:else}{part.text}{/if}{/each}</span></li>
      {/each}
    </ol>
  {/if}
  {#if mechanic.note}<p class="note guide-mechanic__note">{mechanic.note}</p>{/if}
</section>
