<script lang="ts" generics="R extends PublicRun">
  import type { Participant, PublicRun, WeekDay } from '@kanade/api-types';
  import AnswerChip from './AnswerChip.svelte';
  import BossTag from './BossTag.svelte';
  import StatusMark from './StatusMark.svelte';
  import { dayLabel, runFullTitle, sortRuns, tally } from '../format';

  // Works on either projection; the Party column appears only when runs carry participants.
  let { week, onopen }: { week: { timezone: string; days: WeekDay[]; runs: R[] }; onopen?: (run: R) => void } = $props();
  const rows = $derived(sortRuns(week.runs));
  const people = (run: R): { party: string; participants: Participant[] } | null =>
    'participants' in run ? (run as R & { party: string; participants: Participant[] }) : null;
  const withPeople = $derived(week.runs.some((r) => people(r) !== null));
  // Twins read "Ren (2)": their place among same-named members, never the id.
  const twin = (all: Participant[], p: Participant): string | undefined => {
    const twins = all.filter((o) => o.name === p.name);
    return twins.length > 1 ? `${p.name} (${twins.indexOf(p) + 1})` : undefined;
  };
</script>

<!-- Bosses lead each row as its header (HPK); the machine id never shows. -->
<div class="table-wrap">
  <table>
    <caption>Every run this boss week, in the order they happen. Times are {week.timezone}.</caption>
    <thead>
      <tr>
        <th scope="col">Bosses</th>
        <th scope="col">When</th>
        <th scope="col">Status</th>
        <th scope="col" class="num">On</th>
        {#if withPeople}<th scope="col">Party</th>{/if}
      </tr>
    </thead>
    <tbody>
      {#each rows as run (run.id)}
        {@const t = tally(run)}
        <tr>
          <th scope="row">
            {#if onopen}
              <button type="button" class="linkish" aria-label="Open {runFullTitle(run)}" onclick={() => onopen(run)}>
                <ul class="bosslist">{#each run.bosses as boss (boss.token)}<li><BossTag {boss} /></li>{/each}</ul>
              </button>
            {:else}
              <ul class="bosslist">{#each run.bosses as boss (boss.token)}<li><BossTag {boss} /></li>{/each}</ul>
            {/if}
          </th>
          <td class="mono">{dayLabel(week, run.day)} {run.time ?? 'own time'}</td>
          <td><StatusMark status={run.status} words /></td>
          <td class="num">{t.on}/{t.total}</td>
          {#if withPeople}
            {@const detail = people(run)}
            <td>
              {#if detail}
                <span class="party mono">{detail.party}</span>
                <span class="chips">
                  {#each detail.participants as p (p.id)}
                    <AnswerChip participant={p} label={twin(detail.participants, p)} />
                  {/each}
                </span>
              {/if}
            </td>
          {/if}
        </tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .linkish {
    font: inherit;
    color: inherit;
    text-align: left;
    padding: 0.1rem 0.2rem;
    margin: -0.1rem -0.2rem;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    cursor: pointer;
  }

  .linkish:hover {
    background: var(--accent-wash);
  }

  .party {
    display: block;
    font-size: var(--fs-mini);
    color: var(--dim);
    margin-bottom: 0.25rem;
  }

  td.mono {
    white-space: nowrap;
  }
</style>
