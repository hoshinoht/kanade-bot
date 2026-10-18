<!--
  The Week window's Runs tab (B_WeekRuns): one table filling the panel, a
  44 px row card per run; a row opens the same run pane as the board. In the
  phone frame each row reflows to two lines (`_week.scss`), so the roles stay
  explicit (as FixedPage).
-->
<script lang="ts">
  import type { Run, Week } from '@kanade/api-types';
  import { AnswerChip, Portrait, StatusMark, dayLabel, runFullTitle, sortRuns, twinName, DIFFICULTY_WORDS } from '@kanade/ui';

  let { week, selected = null, onopen }: { week: Week; selected?: string | null; onopen: (run: Run) => void } = $props();
  const rows = $derived(sortRuns(week.runs));
  // The whole row opens the run; the row header's button is the keyboard way in.
  function pick(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-row]');
    if (!row || (event.target as HTMLElement).closest('button')) return;
    const run = week.runs.find((r) => r.id === row.dataset.row);
    if (run) onopen(run);
  }
</script>

<div class="week-runs" data-fid="week-runs">
  <!-- svelte-ignore a11y_no_redundant_roles -->
  <table class="week-runs__table" role="table">
    <caption class="vh">Every run this boss week, in the order they happen. Times are {week.timezone}.</caption>
    <!-- svelte-ignore a11y_no_redundant_roles -->
    <thead data-fid="week-runs-head" role="rowgroup">
      <!-- svelte-ignore a11y_no_redundant_roles -->
      <tr role="row">
        <th role="columnheader" scope="col">Bosses</th>
        <th role="columnheader" scope="col" class="week-runs__when">When</th>
        <th role="columnheader" scope="col" class="week-runs__state">Status</th>
        <th role="columnheader" scope="col" class="week-runs__on">On</th>
        <th role="columnheader" scope="col">Party</th>
        <th role="columnheader" scope="col" class="week-runs__channel">Channel</th>
      </tr>
    </thead>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions, a11y_no_redundant_roles -->
    <tbody onclick={pick} role="rowgroup">
      {#each rows as run (run.id)}
        <!-- svelte-ignore a11y_no_redundant_roles -->
        <tr role="row" class="week-runs__row week-runs__row--{run.status}" class:week-runs__row--active={run.id === selected} data-row={run.id} data-fid="week-runs-row">
          <th role="rowheader" scope="row">
            <button type="button" class="week-runs__open" aria-label="Open {runFullTitle(run)}" aria-current={run.id === selected ? 'true' : undefined} onclick={() => onopen(run)}>
              {#each run.bosses as boss (boss.token)}
                <span class="week-runs__boss">
                  <Portrait {boss} />
                  <span class="week-runs__name">{boss.name}</span>
                  <span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>
                </span>
              {/each}
            </button>
          </th>
          <td role="cell" class="mono week-runs__when">{dayLabel(week, run.day)} {run.time ?? 'own time'}</td>
          <td role="cell" class="week-runs__state"><StatusMark status={run.status} words /></td>
          <td role="cell" class="mono week-runs__on">{run.tally.on}/{run.tally.total}</td>
          <td role="cell" class="week-runs__partycell">
            <span class="week-runs__party">
              {#each run.participants as p (p.id)}<AnswerChip participant={p} label={twinName(run.participants, p)} />{/each}
            </span>
          </td>
          <td role="cell" class="week-runs__channel">{run.channel}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
