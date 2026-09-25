<script lang="ts">
  import type { PublicWeek } from '@kanade/api-types';
  import { DayColumn, RunCardBody, WeekRail, runAccessibleName, sortRuns } from '@kanade/ui';

  let { week }: { week: PublicWeek } = $props();
  const byDay = $derived(week.days.map((day) => sortRuns(week.runs.filter((r) => r.day === day.index))));
</script>

<WeekRail days={week.days} runs={week.runs} />

<!-- Focusable: a busy week scrolls sideways, and keyboard users must reach it (WCAG 2.1.1). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="board" role="region" aria-label="The boss week, by day" tabindex="0">
  {#each week.days as day (day.index)}
    {@const runs = byDay[day.index] ?? []}
    <DayColumn {day} count={runs.length}>
      {#if runs.length === 0}
        <p class="board__none"><span aria-hidden="true">·</span><span class="vh">Nothing on</span></p>
      {:else}
        <ul class="board__runs">
          {#each runs as run (run.id)}
            <li class="runcard runcard--{run.status}">
              <span class="vh">{runAccessibleName(week, run)}</span>
              <span class="runcard__visual" aria-hidden="true"><RunCardBody {run} /></span>
            </li>
          {/each}
        </ul>
      {/if}
    </DayColumn>
  {/each}
</div>
