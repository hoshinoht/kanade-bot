<!--
  The seven-day board (board Week), the admin planner's columns without its
  moves: `DayColumn` per day (empty days collapse to a dashed spine), the
  member's cards in day order.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { DayColumn, sortRuns } from '@kanade/ui';
  import MemberCard from './MemberCard.svelte';

  let {
    week,
    runs,
    memberId,
    selected,
    onopen,
  }: { week: MemberWeek; runs: MemberRun[]; memberId: string; selected: string | null; onopen: (run: MemberRun) => void } = $props();

  const sorted = $derived(sortRuns(runs));
</script>

<div class="board member-board" data-fid="week-board">
  {#each week.days as day (day.index)}
    {@const today = sorted.filter((run) => run.day === day.index)}
    <DayColumn {day} count={today.length}>
      {#if today.length === 0}
        <p class="board__none"><span aria-hidden="true">·</span><span class="vh">Nothing on</span></p>
      {:else}
        <ul class="board__runs" data-fid="week-runs-list">
          {#each today as run (run.id)}
            <li><MemberCard {run} {week} {memberId} selected={run.id === selected} {onopen} data-run={run.id} data-fid="week-card" /></li>
          {/each}
        </ul>
      {/if}
    </DayColumn>
  {/each}
</div>
