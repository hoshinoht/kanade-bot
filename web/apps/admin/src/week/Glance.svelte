<!--
  The Week window's "at a glance" pane while no run is open (WeekRail1, gate
  G3): the next run and its party's answers, then the Inbox and the model.
  The week-wide "still waiting" list lives in the Answers tab and the footer's
  unanswered count (user decision 2026-10-04). From 1200 px wide only; below
  that the footer carries the same facts (O5).
-->
<script lang="ts">
  import type { Answer, Run, Summary, Week } from '@kanade/api-types';
  import { ANSWER_MARKS, BossTag, weekStartLabel } from '@kanade/ui';
  import { memberLabel } from '../names/directory.svelte';
  import { openPlaces } from './waiting';

  let {
    summary,
    week,
    onopen,
  }: {
    summary: Summary | null;
    week: Week;
    onopen: (runId: string) => void;
  } = $props();

  // Who still owes an answer first, then maybe, on and out; board order within each.
  const ORDER: Answer[] = ['waiting', 'maybe', 'yes', 'no'];
  const WORD: Record<Answer, string> = { waiting: 'waiting', maybe: 'maybe', yes: 'on', no: 'out' };
  const next = $derived(summary?.next ?? null);
  // The next run's own facts when it is on the week shown.
  const run = $derived<Run | null>(next ? (week.runs.find((r) => r.id === next.run_id) ?? null) : null);
  const art = $derived(run?.bosses.find((b) => b.art)?.art ?? null);
  const maybe = $derived(run ? run.participants.filter((p) => p.answer === 'maybe').length : 0);
  const party = $derived(run ? ORDER.flatMap((answer) => run.participants.filter((p) => p.answer === answer)) : []);
  const unanswered = $derived(party.filter((p) => p.answer === 'waiting').length);
</script>

<aside class="side-pane week-glance" aria-label="At a glance">
  <div class="week-glance__body">
    {#if next}
      <section class="week-glance__next" aria-labelledby="week-glance-next">
        {#if art}<img class="week-glance__art" src={art} alt="" decoding="async" />{/if}
        <p class="cap week-glance__cap" id="week-glance-next">Next up · <span class="mono">{next.countdown}</span></p>
        <p class="week-glance__time mono">{run ? (run.time ?? 'own time') : next.when}</p>
        {#if run}
          <p class="week-glance__date">{weekStartLabel(week.days[run.day]?.date ?? '')}</p>
          <p class="week-glance__bosses">{#each run.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</p>
          <p class="week-glance__fill mono">
            {run.tally.on}/{run.tally.total} · {openPlaces(run)}{#if maybe} · {maybe} maybe{/if}
          </p>
        {:else}
          <p class="week-glance__bosses">{next.bosses}</p>
          <p class="week-glance__fill mono">{next.on}/{next.total} on</p>
        {/if}
        <button type="button" class="btn week-glance__open" onclick={() => onopen(next.run_id)}>Open sheet</button>
      </section>
    {:else}
      <section class="week-glance__next week-glance__next--none" aria-labelledby="week-glance-next">
        <p class="cap week-glance__cap" id="week-glance-next">Next up</p>
        <p class="week-glance__date">Nothing ahead: every run this week has been and gone.</p>
      </section>
    {/if}

    {#if run && party.length}
      <section class="week-glance__party" aria-labelledby="week-glance-party">
        <h2 class="cap week-glance__head" id="week-glance-party">Party · {run.time ?? 'own time'}</h2>
        <p class="week-glance__hint" class:week-glance__hint--warn={unanswered > 0}>
          {unanswered ? `${unanswered} ${unanswered === 1 ? "hasn't" : "haven't"} answered · ping from the sheet` : 'Everyone has answered'}
        </p>
        <ul class="week-glance__rows">
          {#each party as person (person.id)}
            <li class="week-glance__row week-glance__row--{person.answer}">
              <span class="week-glance__name">{memberLabel(run.participants, person.id)}</span>
              <!-- Words beside the mark, never colour alone. -->
              <span class="week-glance__answer"><span aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span> {WORD[person.answer]}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  </div>

  {#if summary}
    <div class="week-glance__foot">
      <a class="week-glance__fact" class:week-glance__fact--warn={summary.inbox > 0} href="/inbox"
        >Inbox <b class="mono">{summary.inbox ? `${summary.inbox} waiting` : 'clear'}</b></a
      >
      <a class="week-glance__fact" class:week-glance__fact--warn={summary.model.busy} href="/limits" title={summary.model.holder ?? 'nothing is holding it'}
        >Model <b class="mono">{summary.model.busy ? 'busy' : 'free'}</b></a
      >
    </div>
  {/if}
</aside>
