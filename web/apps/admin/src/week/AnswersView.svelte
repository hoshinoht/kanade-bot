<!--
  The Week window's Answers tab (B_WeekAnswers): answers by day as paired
  bars (answered filled, waiting outlined and hatched, the numbers printed
  under every pair, a table on request), then who is still waiting, most
  first. Loaded with its tab.
-->
<script lang="ts">
  import type { Attachment } from 'svelte/attachments';
  import type { Stats, Week } from '@kanade/api-types';
  import { dayLabel } from '@kanade/ui';
  import type { Owed } from './waiting';

  let { stats, week, owed, onmember }: { stats: Stats; week: Week; owed: Owed[]; /** Filters the Runs tab to that member. */ onmember: (id: string) => void } =
    $props();

  const uid = $props.id();
  let table = $state(false);
  const totals = $derived(
    stats.per_day.reduce((acc, d) => ({ answered: acc.answered + d.answered, waiting: acc.waiting + d.waiting }), { answered: 0, waiting: 0 }),
  );
  const top = $derived(Math.max(1, ...stats.per_day.flatMap((d) => [d.answered, d.waiting])));

  // Bar heights go in through CSSOM (a style attribute is blocked by style-src 'self').
  const height = (value: number): Attachment<HTMLElement> => (node) => {
    node.style.height = `${(value / top) * 100}%`;
  };
  const names = (member: Owed) => {
    const shown = member.runs.slice(0, 3).map((r) => `${r.title}${r.maybe ? ' (maybe)' : ''}`);
    return member.runs.length > 3 ? `${shown.join(' · ')} · +${member.runs.length - 3}` : shown.join(' · ');
  };
</script>

<div class="week-answers" data-fid="week-answers">
  <section class="week-answers__card" data-fid="week-chart" aria-labelledby="{uid}-chart">
    <div class="week-answers__head">
      <h2 class="week-answers__title" id="{uid}-chart">Answers by day</h2>
      <p class="week-answers__sum"><b class="mono">{totals.answered}</b> answered · <b class="mono">{totals.waiting}</b> still waiting</p>
      <p class="week-answers__legend" aria-hidden="true">
        <span><span class="week-bar week-bar--answered week-bar--key"></span> answered</span>
        <span><span class="week-bar week-bar--waiting week-bar--key"></span> waiting</span>
      </p>
      <button type="button" class="linklike week-answers__toggle" aria-expanded={table} aria-controls="{uid}-table" onclick={() => (table = !table)}
        >{table ? 'Show as chart' : 'Show as table'}</button
      >
    </div>
    {#if table}
      <div class="table-wrap week-answers__table" id="{uid}-table">
        <table>
          <caption class="vh">Answers by day</caption>
          <thead>
            <tr><th scope="col">Day</th><th scope="col" class="num">Answered</th><th scope="col" class="num">Waiting</th></tr>
          </thead>
          <tbody>
            {#each stats.per_day as d (d.day)}
              <tr><th scope="row">{dayLabel(week, d.day)}</th><td class="num">{d.answered}</td><td class="num">{d.waiting}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <ol class="week-answers__bars" data-fid="week-bars" id="{uid}-table">
        {#each stats.per_day as d (d.day)}
          <li class="week-answers__day">
            <span class="week-answers__pair" aria-hidden="true">
              <span class="week-bar week-bar--answered" {@attach height(d.answered)}></span>
              <span class="week-bar week-bar--waiting" {@attach height(d.waiting)}></span>
            </span>
            <span class="week-answers__dow mono">{dayLabel(week, d.day)}</span>
            <span class="week-answers__nums"
              ><b class="mono week-answers__a">{d.answered}</b> · <b class="mono week-answers__w">{d.waiting}</b><span class="vh">
                answered, waiting</span
              ></span
            >
          </li>
        {/each}
      </ol>
    {/if}
  </section>

  <section class="week-answers__card week-answers__card--fill" data-fid="week-waiting" aria-labelledby="{uid}-waiting">
    <div class="week-answers__head">
      <h2 class="week-answers__title" id="{uid}-waiting">Still waiting</h2>
      <p class="week-answers__sub">by member · most first</p>
    </div>
    {#if owed.length}
      <ul class="week-answers__members">
        {#each owed as member (member.id)}
          <li>
            <button type="button" class="week-answers__member" data-fid="week-waiting-row" title="Show {member.name}'s runs" onclick={() => onmember(member.id)}>
              <b class="week-answers__name">{member.name}</b>
              <span class="week-answers__runs">{names(member)}</span>
              <span class="week-count mono"><span class="vh">owes </span>{member.runs.length}</span>
            </button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="week-answers__sub">Everybody has answered every run ahead.</p>
    {/if}
  </section>
</div>
