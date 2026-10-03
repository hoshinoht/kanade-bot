<!--
  The Week window's "at a glance" pane while no run is open (WeekRail1, gate
  G3): what is next, who still owes answers, then the Inbox and the model.
  From 1200 px wide only; below that the footer carries the same facts (O5).
-->
<script lang="ts">
  import type { Run, Summary, Week } from '@kanade/api-types';
  import { BossTag, weekStartLabel } from '@kanade/ui';
  import type { Owed } from './waiting';
  import { openPlaces } from './waiting';

  let {
    summary,
    week,
    owed,
    waiting,
    onopen,
    onanswers,
  }: {
    summary: Summary | null;
    week: Week;
    owed: Owed[];
    /** Answers still owed this week (the Answers tab's count). */
    waiting: number | null;
    onopen: (runId: string) => void;
    /** Shows the Answers tab, where every waiting member is listed. */
    onanswers: () => void;
  } = $props();

  const SHOWN = 4;
  const next = $derived(summary?.next ?? null);
  // The next run's own facts when it is on the week shown.
  const run = $derived<Run | null>(next ? (week.runs.find((r) => r.id === next.run_id) ?? null) : null);
  const art = $derived(run?.bosses.find((b) => b.art)?.art ?? null);
  const maybe = $derived(run ? run.participants.filter((p) => p.answer === 'maybe').length : 0);
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

    <section class="week-glance__waiting" aria-labelledby="week-glance-waiting">
      <h2 class="cap week-glance__head" id="week-glance-waiting">
        Waiting on answers {#if waiting !== null}<span class="week-glance__total mono">{waiting}</span>{/if}
      </h2>
      {#if owed.length}
        <ul class="week-glance__rows">
          {#each owed.slice(0, SHOWN) as member (member.id)}
            <li class="week-glance__row">
              <span class="week-glance__name">{member.name}</span>
              <span class="week-count mono" aria-label="{member.runs.length} unanswered">{member.runs.length}</span>
            </li>
          {/each}
        </ul>
        {#if owed.length > SHOWN}
          <button type="button" class="linklike week-glance__more" onclick={onanswers}>+{owed.length - SHOWN} more in Answers</button>
        {/if}
      {:else}
        <p class="week-glance__none">Everybody has answered every run ahead.</p>
      {/if}
    </section>
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
