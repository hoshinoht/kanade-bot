<!--
  The Week window's pane while no run is open (board Main, `glance`): the
  admin "at a glance" pane (`week/Glance.svelte`, `.week-glance`) for one
  member: their next run with its art, countdown and party, then Needs you
  (answers they owe, either week). Asking for a change and the calendar feed
  are drawn as the board draws them, marked "coming soon" and disabled.
-->
<script lang="ts">
  import type { Answer, MemberRun, MemberWeek } from '@kanade/api-types';
  import { ANSWER_MARKS, BossArt, BossTag, Icon, openPlaces, runCountdown, runTitle, STATUS_WORDS, WavyProgress, weekStartLabel } from '@kanade/ui';
  import { answersOwed, countdownWords, nextOwn, youFirst } from '../member';
  import type { WeekKey } from '../weeks.svelte';

  let {
    current,
    next,
    memberId,
    onopen,
  }: {
    current: MemberWeek;
    next: MemberWeek | null;
    memberId: string;
    onopen: (run: MemberRun, which: WeekKey) => void;
  } = $props();

  const WORD: Record<Answer, string> = { waiting: 'waiting', maybe: 'maybe', yes: 'on', no: 'out' };
  // The next run is this week's, else next week's first.
  const lead = $derived.by(() => {
    const here = nextOwn(current);
    if (here) return { run: here, week: current, which: 'this' as WeekKey };
    const there = next ? nextOwn(next) : null;
    return there && next ? { run: there, week: next, which: 'next' as WeekKey } : null;
  });
  const art = $derived(lead?.run.bosses.find((b) => b.art) ?? null);
  const countdown = $derived(lead ? runCountdown(lead.run, lead.week) : null);
  const party = $derived(lead ? youFirst(lead.run, memberId) : []);
  const unanswered = $derived(party.filter((p) => p.answer === 'waiting').length);
  const owed = $derived([
    ...answersOwed(current.runs, memberId).map((run) => ({ run, week: current, which: 'this' as WeekKey })),
    ...(next ? answersOwed(next.runs, memberId).map((run) => ({ run, week: next, which: 'next' as WeekKey })) : []),
  ]);
</script>

<aside class="side-pane week-glance member-glance" aria-label="Your week" data-fid="glance">
  <div class="week-glance__body">
    {#if lead}
      {@const run = lead.run}
      <section class="week-glance__next" aria-labelledby="your-next" data-fid="glance-next">
        {#if art}<BossArt class="week-glance__art" still={art.art} animated={art.animated} />{/if}
        <p class="cap week-glance__cap" id="your-next">Your next run{#if countdownWords(run, lead.week)} · <span class="mono">{countdownWords(run, lead.week)}</span>{/if}</p>
        <p class="week-glance__time mono">{run.time ?? 'own time'}</p>
        {#if countdown}
          <WavyProgress
            class="week-glance__countdown"
            value={countdown.value}
            max={countdown.max}
            wavy={countdown.wavy}
            ticks={countdown.ticks}
            label="Countdown to {runTitle(run)}"
            text={countdown.text}
          />
        {/if}
        <p class="week-glance__date">{weekStartLabel(lead.week.days[run.day]?.date ?? '')}</p>
        <p class="week-glance__bosses">{#each run.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</p>
        <p class="week-glance__fill mono">{run.tally.on}/{run.tally.total} · {run.status === 'at_risk' ? STATUS_WORDS.at_risk : openPlaces(run)}</p>
        <button type="button" class="btn week-glance__open" onclick={() => onopen(run, lead.which)}>Open run</button>
      </section>
    {:else}
      <section class="week-glance__next week-glance__next--none" aria-labelledby="your-next" data-fid="glance-next">
        <p class="cap week-glance__cap" id="your-next">Your next run</p>
        <p class="week-glance__date">Nothing ahead for you this week or next.</p>
      </section>
    {/if}

    {#if lead && party.length}
      <section aria-labelledby="your-party">
        <h2 class="cap week-glance__head" id="your-party">Party · {lead.run.time ?? 'own time'}</h2>
        <p class="week-glance__hint" class:week-glance__hint--warn={unanswered > 0}>
          {unanswered ? `${unanswered} ${unanswered === 1 ? "hasn't" : "haven't"} answered yet` : 'Everyone has answered'}
        </p>
        <ul class="week-glance__rows" data-fid="week-party">
          {#each party as person (person.id)}
            <li class="week-glance__row week-glance__row--{person.answer}">
              <span class="week-glance__name">{person.id === memberId ? 'You' : person.name}</span>
              <!-- Words beside the mark, never colour alone. -->
              <span class="week-glance__answer"><span aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span> {WORD[person.answer]}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    <section aria-labelledby="needs-you">
      <h2 class="cap week-glance__head" id="needs-you">Needs you</h2>
      <ul class="week-glance__rows member-glance__needs" data-fid="needs-you">
        {#if owed.length}
          {@const first = owed[0]!}
          <li>
            <button type="button" class="week-glance__fact week-glance__fact--warn" onclick={() => onopen(first.run, first.which)}
              ><span>Answer owed · <span class="mono">{first.week.days[first.run.day]?.dow ?? ''} {first.run.time ?? 'own time'}</span></span><b class="mono"
                >{owed.length}</b
              ></button
            >
          </li>
        {:else}
          <li class="week-glance__fact member-glance__clear"><span>Nothing owed: you've answered every run</span><Icon name="check" /></li>
        {/if}
      </ul>
    </section>
  </div>

  <div class="week-glance__foot" data-fid="glance-foot">
    <button type="button" class="week-glance__fact member-glance__soon" aria-disabled="true"
      ><span>Ask for a change…</span><span class="status-chip status-chip--warn">coming soon</span></button
    >
    <button type="button" class="week-glance__fact member-glance__soon" aria-disabled="true"
      ><span>Add my runs to my calendar</span><span class="status-chip status-chip--warn">coming soon</span></button
    >
  </div>
</aside>
