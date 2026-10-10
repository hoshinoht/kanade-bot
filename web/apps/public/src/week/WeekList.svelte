<!--
  The Week window's List tab (boards WeekList, WeekList-Selected): the admin
  Runs table (`week/RunsTable.svelte`, `_week.scss` `.week-runs`) grouped
  under day rows, each run on one line (stacked portraits, the bosses'
  names, their pills), with the member's own/other treatment: own rows full
  with YOU and the member's answer, everyone else's dashed and "view only".
  A row opens the same pane.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek, Participant } from '@kanade/api-types';
  import { ANSWER_MARKS, AnswerChip, BossStack, longDate, runFullTitle, sortRuns, StatusMark } from '@kanade/ui';
  import { yours } from '../member';

  let {
    week,
    runs,
    memberId,
    selected,
    onopen,
  }: { week: MemberWeek; runs: MemberRun[]; memberId: string; selected: string | null; onopen: (run: MemberRun) => void } = $props();

  const rows = $derived(sortRuns(runs));
  // The member reads as "You"; twins read "Ren (2)": their place among same-named members, never the id.
  const label = (all: Participant[], p: Participant): string | undefined => {
    if (p.id === memberId) return 'You';
    const twins = all.filter((o) => o.name === p.name);
    return twins.length > 1 ? `${p.name} (${twins.indexOf(p) + 1})` : undefined;
  };

  // The whole row opens the run; the row header's button is the keyboard way in.
  function pick(event: MouseEvent) {
    const row = (event.target as HTMLElement).closest<HTMLElement>('[data-row]');
    if (!row || (event.target as HTMLElement).closest('button')) return;
    const run = runs.find((r) => r.id === row.dataset.row);
    if (run) onopen(run);
  }
</script>

<div class="week-runs" data-fid="week-runs">
  <table class="week-runs__table">
    <caption class="vh">Every run this boss week, in the order they happen. Times are {week.timezone}.</caption>
    <thead data-fid="week-runs-head">
      <tr>
        <th scope="col">Bosses</th>
        <th scope="col" class="week-runs__when">When</th>
        <th scope="col" class="week-runs__state">Status</th>
        <th scope="col" class="week-runs__on">On</th>
        <th scope="col">Party</th>
        <th scope="col" class="week-runs__you">You</th>
      </tr>
    </thead>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <tbody onclick={pick}>
      {#each week.days as day (day.index)}
        {@const today = rows.filter((r) => r.day === day.index)}
        {#if today.length}
          <tr class="week-runs__dayrow">
            <th scope="colgroup" colspan="6" class="week-runs__day"><b>{day.dow}</b> {longDate(day.date)}{day.is_reset ? ' · reset' : ''}{day.is_today ? ' · today' : ''}</th>
          </tr>
        {/if}
        {#each today as run (run.id)}
          {@const answer = yours(run, memberId)?.answer}
          <tr
            class="week-runs__row week-runs__row--{run.status}"
            class:week-runs__row--mine={run.mine}
            class:week-runs__row--other={!run.mine}
            class:week-runs__row--active={run.id === selected}
            data-row={run.id}
            data-fid="week-runs-row"
          >
            <th scope="row">
              <button
                type="button"
                class="week-runs__open"
                aria-label="Open {runFullTitle(run)}{run.mine ? '' : ', view only'}"
                aria-current={run.id === selected ? 'true' : undefined}
                onclick={() => onopen(run)}
              >
                <BossStack bosses={run.bosses} />
              </button>
            </th>
            <td class="mono week-runs__when">{run.time ?? 'own time'}</td>
            <td class="week-runs__state"><StatusMark status={run.status} words /></td>
            <td class="mono week-runs__on">{run.tally.on}/{run.tally.total}</td>
            <td>
              <span class="week-runs__party member-list__party">
                {#each run.participants as p (p.id)}<AnswerChip participant={p} label={label(run.participants, p)} />{/each}
              </span>
            </td>
            <td class="week-runs__you">
              {#if answer}
                <span class="you-chip">YOU</span>
                <span class="member-list__answer">{answer === 'waiting' ? 'not answered' : ANSWER_MARKS[answer].word}</span>
              {:else}
                <span class="member-card__view">view only</span>
              {/if}
            </td>
          </tr>
        {/each}
      {/each}
    </tbody>
  </table>
</div>
