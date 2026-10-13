<!--
  One member's readiness (module `character_data`): the level and Combat Power
  (dropped for the time of a MapleScouter reading, which replaces it) with
  their source, then the level verdict per boss with that boss's MapleScouter
  chip ("· crit assumed" when MapleScouter assumed a 100% critical rate; the
  full label in its title). Shown, never judged, never blocking. Drawn by the run sheet's Party tab
  (B_CharReady, B_PhoneCharReady) and the Fixed editor's Characters tab
  (B_FixedReady, B_PhoneFixedReady); `fid` and `bossesFid` are their tags.
-->
<script lang="ts">
  import type { CharacterChoice, ReadinessSlot } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { dayTime, levelChip, readinessLine, scouterAt, scouterChip } from './format';

  let {
    slot,
    character,
    timeZone,
    bossLabel,
    fid,
    bossesFid,
    row = false,
  }: {
    slot: ReadinessSlot;
    /** The slot's character (the caller draws this only with one). */
    character: CharacterChoice;
    timeZone: string;
    /** The boss as the screen names it ("HCarling"); named only when there are several. */
    bossLabel: (key: string, difficulty: string) => string;
    fid: string;
    bossesFid?: string;
    /** The values line and the bosses side by side, wrapping (a wide Fixed editor). */
    row?: boolean;
  } = $props();

  const line = $derived(readinessLine(slot));
  const readAt = $derived(scouterAt(slot));
</script>

<div class="ready" class:ready--row={row} role="group" aria-label="Readiness of {character.ign}" data-fid={fid}>
  <span class="ready__line"
    ><span class="ready__values">{line.values}</span><span class="ready__source">{line.source}</span>{#if readAt}<span class="ready__source ready__scouter-at"
        >MapleScouter · <span class="mono">{dayTime(readAt, timeZone)}</span></span
      >{/if}</span
  >
  <ul class="ready__bosses" data-fid={bossesFid}>
    {#each slot.bosses as boss (`${boss.boss}-${boss.difficulty}`)}
      {@const chip = levelChip(boss)}
      <li class="ready__boss">
        {#if slot.bosses.length > 1}<span class="ready__boss-name">{bossLabel(boss.boss, boss.difficulty)}</span>{/if}
        <StatusChip tone={chip.tone}>{chip.text}</StatusChip>{#if boss.scouter}{@const est = scouterChip(boss.scouter)}<span
            class="status-chip status-chip--neutral ready__scouter"
            title={est.assumed ? `${est.rating} · ${est.assumed}` : est.rating}
            >{est.text}<span class="vh"> · {est.rating}{est.assumed ? ` · ${est.assumed}` : ''}</span></span
          >{/if}
      </li>
    {/each}
  </ul>
</div>
