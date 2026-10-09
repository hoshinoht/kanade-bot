<!--
  An open run on a phone (boards PhoneRun, PhoneRunOther): its own screen,
  the top bar's "‹ Week" going back. The admin phone sheet's hero (clock,
  bosses, chips) and a Party window; the member's own run shows their answer
  (read-only for now) and Move / Ask drawn disabled ("coming soon"); anyone
  else's run a lock box and "Ask to join" in a foot bar, disabled. Its
  parts come in forward (`enter`), as the admin's list-detail screens do on
  a phone; the Week comes back backward (WeekPage).
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { ANSWER_MARKS, answerCounts, Avatar, BossArt, BossTag, dayLabel, enter, Icon, StatusMark } from '@kanade/ui';
  import { countdownWords, yours, youFirst } from '../member';
  import AnswerSoon from './AnswerSoon.svelte';

  let { run, week, memberId }: { run: MemberRun; week: MemberWeek; memberId: string } = $props();

  const counts = $derived(answerCounts(run.participants));
  const art = $derived(run.bosses.find((b) => b.art) ?? null);
  const soon = $derived(countdownWords(run, week));
  const mine = $derived(yours(run, memberId));
  // The enter replays only when the run itself changes: a fresh read hands in a new object for the same run.
  const runId = $derived(run.id);
</script>

<h1 class="vh">{run.mine ? 'Your run' : 'View only'}: {run.bosses.map((b) => b.name).join(' + ')}, {dayLabel(week, run.day)} {run.time ?? 'own time'}</h1>
<section class="member-hero member-hero--{run.status}" class:member-hero--other={!run.mine} aria-label="Run" data-fid="sheet-hero" {@attach enter(runId)}>
  {#if art}<BossArt class="member-hero__art" still={art.art} animated={art.animated} />{/if}
  <div class="member-hero__when" data-fid="sheet-when">
    <span class="member-hero__time mono">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
    <span class="cap">{dayLabel(week, run.day)}{soon ? ` · ${soon}` : ''}</span>
  </div>
  <ul class="member-hero__bosses">
    {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait /></li>{/each}
  </ul>
  <div class="member-hero__chips" data-fid="sheet-chips">
    <StatusMark status={run.status} words pill />
    <span class="tone tone--neutral"><b class="mono">{run.tally.on}/{run.tally.total}</b>&nbsp;on</span>
    {#if counts.no}<span class="tone tone--danger mono">{counts.no} out</span>{/if}
    <span class="tone tone--neutral mono">#{run.channel}</span>
  </div>
  {#if mine}
    <AnswerSoon answer={mine.answer} data-fid="week-answer" />
    {#if run.can_edit}
      <button type="button" class="btn btn--primary btn--key member-hero__full" aria-disabled="true"><Icon name="move" />Move this week… <span class="status-chip status-chip--warn">coming soon</span></button>
    {/if}
    <button type="button" class="btn member-hero__full" aria-disabled="true">Ask for another change… <span class="status-chip status-chip--warn">coming soon</span></button>
  {:else}
    <p class="member-lock" data-fid="week-lock"><Icon name="lock" /><span>You're not in this run, so you can't answer or move it.</span></p>
  {/if}
</section>
<section class="card tabs window-fill member-hero__window" aria-label="Run detail" data-fid="sheet-window" {@attach enter(runId)}>
  <div class="card__head tabs__strip">
    <div class="tabs__tabs" role="tablist" aria-label="Run detail" data-fid="sheet-tabs">
      <button type="button" role="tab" class="tabs__tab" id="phone-run-party" aria-selected="true" aria-controls="phone-run-panel" tabindex="0"
        >Party<span class="tabs__count">{counts.total}</span></button
      >
    </div>
  </div>
  <div class="member-hero__panel" id="phone-run-panel" role="tabpanel" aria-labelledby="phone-run-party" tabindex="-1">
    <ul class="member-slots" data-fid="week-party">
      {#each youFirst(run, memberId) as person (person.id)}
        <li class="member-slot" class:member-slot--me={person.id === memberId}>
          <Avatar class="member-slot__avatar" src={null} name={person.name} />
          <span class="member-slot__name">{person.id === memberId ? 'You' : person.name}</span>
          <span class="chip chip--{person.answer}"><span aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span> {ANSWER_MARKS[person.answer].word}</span>
        </li>
      {/each}
    </ul>
  </div>
</section>
{#if !run.mine}
  <div class="member-hero__foot" data-fid="sheet-foot" {@attach enter(runId)}>
    <button type="button" class="btn btn--primary btn--key member-hero__full" aria-disabled="true">Ask to join… <span class="status-chip status-chip--warn">coming soon</span></button>
    <p class="field__hint">A request goes to the admins; the party doesn't change until one approves it.</p>
  </div>
{/if}
