<!--
  The run open beside the board (boards Week-RunMine, Week-RunOther,
  WeekList-Selected, ConfirmAnswer): the admin run pane (`RunSheet.svelte`
  pane, `.week-pane`) as the member sees it. Their own run: "YOU'RE IN", the
  art header, their answer (In / Maybe / Out, live), Move to (this boss week
  only, while the run can still move) and "Ask for another change…". Anyone
  else's run: view only, a lock box saying why, the party, and "Ask to
  join…". Both asks open the request form. It enters forward (`enter`) and
  leaves through `is-leaving` while the page's `Presence` keeps it, as the
  admin's side panes do.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { ANSWER_MARKS, answerCounts, AnswerBar, BossArt, BossTag, dayLabel, enter, Icon, runCountdown, runFullTitle, StatusMark, WavyProgress } from '@kanade/ui';
  import { countdownWords, yours } from '../member';
  import MemberMove from '../move/MemberMove.svelte';
  import type { Route } from '../route.svelte';
  import type { WeekKey } from '../weeks.svelte';
  import AnswerChoice from '../writes/AnswerChoice.svelte';
  import type { RunFlow } from '../writes/flow.svelte';
  import { follow } from '../writes/follow';
  import { askPath, movable, type Choice } from '../writes/runs';

  let {
    run,
    week,
    which,
    memberId,
    flow,
    route,
    picked = null,
    onpress,
    leaving = false,
    onleft,
    onclose,
  }: {
    run: MemberRun;
    week: MemberWeek;
    which: WeekKey;
    memberId: string;
    flow: RunFlow;
    route: Route;
    /** The answer a fresh sign-in came back with, not saved yet. */
    picked?: Choice | null;
    onpress?: () => void;
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
    onclose: () => void;
  } = $props();

  const counts = $derived(answerCounts(run.participants));
  const art = $derived(run.bosses.find((b) => b.art) ?? null);
  const until = $derived(runCountdown(run, week));
  const soon = $derived(countdownWords(run, week));
  const mine = $derived(yours(run, memberId));
  const title = $derived(`${runFullTitle(run)}, ${dayLabel(week, run.day)} ${run.time ?? 'own time'}`);
  // The enter replays only when the run itself changes: a fresh read hands in a new object for the same run.
  const runId = $derived(run.id);
  const ask = $derived(askPath(run));
</script>

<aside
  class="side-pane week-pane run--{run.status} member-pane"
  class:is-leaving={leaving}
  inert={leaving}
  aria-label="{run.mine ? 'Your run' : 'View only'} · {title}"
  data-fid="week-pane"
  onanimationend={onleft}
  {@attach enter(runId)}
>
  <div class="member-pane__head" data-fid="week-pane-head">
    {#if run.mine}
      <p class="member-pane__who"><span class="you-chip">YOU'RE IN</span> Your run</p>
    {:else}
      <p class="member-pane__who"><span class="member-card__view">not in this run</span> View only</p>
    {/if}
    <button type="button" class="btn btn--ghost member-pane__close" aria-label="Close {title}" onclick={onclose}><Icon name="x" /></button>
  </div>
  <div class="member-pane__panel">
    <header class="member-pane__art" data-fid="week-pane-id">
      {#if art}
        <div class="run__arts run__arts--1" data-fid="week-pane-art">
          <span class="run__slice"><BossArt class="run__art" still={art.art} animated={art.animated} /></span>
        </div>
      {/if}
      <div class="member-pane__lead">
        <p class="member-pane__when">
          <span class="member-pane__time mono">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
          <span class="cap member-pane__day">{dayLabel(week, run.day)}{#if soon} · {soon}{/if}</span>
        </p>
        {#if until}
          <WavyProgress
            class="wavy--inline member-pane__countdown"
            value={until.value}
            max={until.max}
            wavy={until.wavy}
            ticks={until.ticks}
            label="Countdown to {runFullTitle(run)}"
            text={until.text}
          />
        {/if}
        <ul class="member-pane__bosses">
          {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait level /></li>{/each}
        </ul>
        <p class="member-pane__chips">
          <StatusMark status={run.status} words pill />
          <span class="tone tone--neutral"><b class="mono">{run.tally.on}/{run.tally.total}</b>&nbsp;on</span>
          {#if counts.no}<span class="tone tone--danger mono">{counts.no} out</span>{/if}
          {#if counts.maybe}<span class="tone tone--info">{counts.maybe} maybe</span>{/if}
          {#if counts.waiting}<span class="tone tone--neutral">{counts.waiting} waiting</span>{/if}
          <span class="tone tone--neutral mono">{run.channel}</span>
        </p>
        <AnswerBar participants={run.participants} class="member-pane__answers" />
      </div>
    </header>
    <div class="member-pane__body">
      {#if mine}
        <section class="member-pane__sec" aria-labelledby="member-answer-title">
          <p class="cap" id="member-answer-title">Your answer</p>
          <AnswerChoice {run} {week} {which} {memberId} {flow} {picked} {onpress} data-fid="week-answer" />
        </section>
        {#if movable(run, week, which, memberId)}
          <MemberMove
            {run}
            {week}
            {memberId}
            {flow}
            variant="pane"
            step={30}
            legend="Move to"
            stepHint="↑ ↓ step 30 min · this week only{run.fixed_id ? ', your weekly timing stays the same' : ''}"
            fids={{ time: 'move-time', type: 'move-type' }}
            onmoved={(result) => flow.moved(result, week)}
            data-fid="week-move"
          />
        {/if}
      {:else}
        <p class="member-lock" data-fid="week-lock"><Icon name="lock" /><span>You're not in this run, so you can't answer or move it.</span></p>
        <!-- Their party; on the member's own run the answer bar above and Your week carry it (board Week-RunMine). -->
        <p class="cap member-pane__label">Party · {run.party}</p>
        <ul class="week-glance__rows" data-fid="week-party">
          {#each run.participants as person (person.id)}
            <li class="week-glance__row week-glance__row--{person.answer}">
              <span class="week-glance__name">{person.name}</span>
              <span class="week-glance__answer"><span aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span> {ANSWER_MARKS[person.answer].word}</span>
            </li>
          {/each}
        </ul>
      {/if}
      <div class="member-pane__ask">
        <a class="btn" href={ask} onclick={(event) => follow(route, event, ask)}>{run.mine ? 'Ask for another change… (leave, swap, weekly)' : 'Ask to join…'}</a>
      </div>
      {#if !run.mine}<p class="field__hint">A request goes to the admins; the party doesn't change until one approves it.</p>{/if}
    </div>
  </div>
</aside>
