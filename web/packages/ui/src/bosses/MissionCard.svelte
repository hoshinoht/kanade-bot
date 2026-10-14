<!--
  A Destiny / Champion mission at the selected difficulty: series and place,
  the modifier (direction in words, not only colour), the series track from
  the API (just "Mission N" until it lists the stops; champion stops by rank
  letter, only the ranks tracked), needs and rules (solo or not is a rule).
-->
<script lang="ts">
  import type { Mission, MissionStop } from '@kanade/api-types';
  import StatusChip from '../components/StatusChip.svelte';
  import { LETTER, SERIES_NAME, missionPlace, stopLabel } from './guide';

  let { mission, stops, difficulty, fid }: { mission: Mission; stops: MissionStop[]; difficulty: string; /** Fidelity tag (member portal). */ fid?: string } = $props();

  const track = $derived(stops.filter((stop) => stop.series === mission.series).sort((left, right) => left.order - right.order));
  const place = $derived(missionPlace(mission.series, mission.order, track.length));
</script>

<section class="guide-mission" aria-labelledby="guide-mission-heading" data-fid={fid}>
  <div class="guide-mission__head">
    <div>
      <p class="cap">{SERIES_NAME[mission.series]} mission · {place}</p>
      <h3 id="guide-mission-heading">{mission.title}</h3>
    </div>
    <span class="boss-tick boss-tick--{LETTER[difficulty] ?? difficulty.toLowerCase()}">{difficulty.toUpperCase()}</span>
  </div>
  {#if mission.modifier}
    <p class="guide-mission__mod guide-mission__mod--{mission.modifier.direction}">
      <span aria-hidden="true">{mission.modifier.direction === 'up' ? '▲' : '▼'}</span>
      <strong>{mission.modifier.text}</strong>
      <span class="guide-mission__dir">{mission.modifier.direction === 'up' ? 'in your favour' : 'against you'}</span>
    </p>
  {/if}
  {#if track.length}
    <ol class="guide-mission__track guide-mission__track--n{track.length}" aria-label="Mission order">
      {#each track as stop (stop.key)}
        {@const here = stop.order === mission.order}
        <li class:guide-mission__here={here} aria-current={here ? 'step' : undefined}>
          <span class="guide-mission__num mono">{stopLabel(stop.series, stop.order)}{here ? ' · now' : ''}</span>{stop.name}
        </li>
      {/each}
    </ol>
  {/if}
  {#if mission.needs || mission.rules?.length}
    <ul class="guide-mission__chips">
      {#if mission.needs}<li><StatusChip>Needs {mission.needs}</StatusChip></li>{/if}
      {#each mission.rules ?? [] as rule (rule)}<li><StatusChip>{rule}</StatusChip></li>{/each}
    </ul>
  {/if}
</section>
