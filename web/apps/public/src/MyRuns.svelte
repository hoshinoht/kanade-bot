<!--
  My runs (boards MyRuns, MyRuns-Next, PhoneMyRuns, MyRuns-Timings-Owner,
  PhoneMyRuns-Timings-Owner): the member's own runs from this week's and
  next week's `MemberWeek`, and the weekly timings they are on, as title-bar
  tabs This week n / Next week n / Weekly timings n (`?week=next`,
  `?week=timings`). Each run is a card with its art veil: the time and day,
  bosses (`BossStack`), channel · fill · teammates, and the member's answer,
  read-only for now. Weekly timings carry their ownership (`timings/`).
  What changed comes later. Beside the runs (wide screens): this week's
  counts and the calendar feed, marked "coming soon".
-->
<script lang="ts">
  import type { MemberRun, MemberWeek, PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { ANSWER_MARKS, BossArt, BossStack, dayLabel, dayNumber, Icon, LoadingState, longDate, sortRuns, STATUS_WORDS, type Toaster } from '@kanade/ui';
  import { tick, untrack, type Snippet } from 'svelte';
  import { answersOwed, countdownWords, isPast, youFirst, yours } from './member';
  import type { Route } from './route.svelte';
  import { OwnerFlow } from './timings/flow.svelte';
  import OwnerDialogs from './timings/OwnerDialogs.svelte';
  import type { MemberTimingsList } from './timings/timings.svelte';
  import TimingsPanel from './timings/TimingsPanel.svelte';
  import type { MemberWeeks, WeekKey } from './weeks.svelte';
  import AnswerSoon from './week/AnswerSoon.svelte';

  let {
    weeks,
    timings,
    route,
    session,
    current,
    toaster,
    phone,
    notice,
  }: {
    weeks: MemberWeeks;
    timings: MemberTimingsList;
    route: Route;
    session: PublicSession;
    /** This device's row in the session list, for when it signed in (Confirm it's you). */
    current: PublicSessionRow | null;
    toaster: Toaster;
    phone: boolean;
    notice?: Snippet;
  } = $props();

  type Tab = WeekKey | 'timings';
  const TABS: { id: Tab; label: string; short: string }[] = [
    { id: 'this', label: 'This week', short: 'This week' },
    { id: 'next', label: 'Next week', short: 'Next' },
    { id: 'timings', label: 'Weekly timings', short: 'Timings' },
  ];
  const memberId = $derived(session.member.id);
  const tab: Tab = $derived(TABS.find((t) => t.id === route.params.get('week'))?.id ?? 'this');
  // Weekly timings has no week of its own: the page line and its range stay this week's.
  const which: WeekKey = $derived(tab === 'timings' ? 'this' : tab);
  const week = $derived(weeks.week(which));
  const own = (w: MemberWeek | null) => (w ? sortRuns(w.runs.filter((r) => r.mine)) : []);
  const runs = $derived(own(week));
  const ahead = $derived(runs.filter((r) => !isPast(r)));
  const done = $derived(runs.filter(isPast));
  const owed = $derived(week ? answersOwed(week.runs, memberId) : []);
  const range = $derived.by(() => {
    const first = week?.days[0];
    const last = week?.days[week.days.length - 1];
    return first && last ? `${first.dow} ${dayNumber(first.date)} – ${last.dow} ${longDate(last.date)}` : '';
  });
  const counts = $derived<Record<Tab, number | null>>({
    this: weeks.this ? own(weeks.this).length : null,
    next: weeks.next ? own(weeks.next).length : null,
    timings: timings.data?.timings.length ?? null,
  });
  const tabs: Partial<Record<Tab, HTMLButtonElement>> = {};
  const flow = untrack(() => new OwnerFlow(timings, toaster));

  // Every visit reads the timings afresh: other members ask and decide meanwhile.
  $effect(() => untrack(() => void timings.load()));

  function choose(next: Tab, focus = false) {
    route.set({ week: next === 'this' ? '' : next });
    if (focus) void tick().then(() => tabs[next]?.focus());
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const to = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    choose(TABS[(to + TABS.length) % TABS.length]!.id, true);
  }
</script>

{#snippet card(run: MemberRun, w: MemberWeek)}
  {@const me = yours(run, memberId)}
  {@const art = run.bosses.find((b) => b.art)}
  {@const soon = countdownWords(run, w)}
  {@const mates = run.participants.filter((p) => p.id !== memberId).map((p) => p.name)}
  <article
    class="member-run member-run--{run.status}"
    class:member-run--owed={me?.answer === 'waiting' && run.can_edit}
    class:member-run--done={isPast(run)}
    aria-label="{run.bosses.map((b) => b.token).join(' + ')}, {dayLabel(w, run.day)} {run.time ?? 'own time'}"
    data-fid="myruns-row"
  >
    {#if art}<BossArt class="member-run__art" still={art.art} animated={art.animated} />{/if}
    <div class="member-run__clock">
      <span class="member-run__time mono">{run.time ?? 'own time'}</span>
      <span class="cap">{dayLabel(w, run.day)}{soon ? ` · ${soon}` : ''}</span>
    </div>
    <div class="member-run__mid">
      <BossStack bosses={run.bosses} />
      <p class="member-run__facts">
        {#if me?.answer === 'waiting' && run.can_edit}
          <span class="status-chip status-chip--warn">answer owed</span>
        {:else}
          <span class="status-chip" class:status-chip--risk={run.status === 'at_risk'}>{STATUS_WORDS[run.status]}</span>
        {/if}
        <span class="account-row__sub">#{run.channel} · {run.tally.on} of {run.tally.total} on{mates.length ? ` · with ${mates.join(', ')}` : ''}</span>
      </p>
      <p class="member-run__party">
        {#each youFirst(run, memberId) as person (person.id)}
          <span class="chip chip--{person.answer}" class:chip--me={person.id === memberId}
            ><span aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span> {person.id === memberId ? 'You' : person.name}<span class="vh">
              ({ANSWER_MARKS[person.answer].word})</span
            ></span
          >
        {/each}
      </p>
    </div>
    {#if me && !phone}
      <div class="member-run__acts">
        {#if isPast(run)}
          <p class="member-run__was">You were {me.answer === 'yes' ? 'In' : me.answer === 'no' ? 'Out' : me.answer === 'maybe' ? 'Maybe' : 'unanswered'}</p>
        {:else}
          <AnswerSoon answer={me.answer} hint={false} />
        {/if}
      </div>
    {/if}
  </article>
{/snippet}

{#snippet list()}
  {#if !week}
    <LoadingState text="Loading your runs…" />
  {:else if runs.length === 0}
    <p class="note member-runs__none">You're not in any run {which === 'next' ? 'next week' : 'this week'}.</p>
  {:else}
    {#if ahead.length}
      <h3 class="cap member-runs__cap">Coming up</h3>
      <div class="member-runs__group">{#each ahead as run (run.id)}{@render card(run, week)}{/each}</div>
    {/if}
    {#if done.length}
      <h3 class="cap member-runs__cap">Done {which === 'next' ? 'next week' : 'this week'}</h3>
      <div class="member-runs__group">{#each done as run (run.id)}{@render card(run, week)}{/each}</div>
    {/if}
    <p class="field__hint member-runs__hint">
      <span class="status-chip status-chip--warn">coming soon</span> Answering and moving here; react on the run's card in Discord for now.
    </p>
  {/if}
{/snippet}

{#snippet strip()}
  <div class="card__head tabs__strip" class:phone-tabs={phone} data-fid="window-bar">
    <h2 class="vh" id="mine-title">My runs</h2>
    <div class="tabs__tabs" role="tablist" aria-label="My runs" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        <button
          class="tabs__tab"
          role="tab"
          type="button"
          id="mine-tab-{t.id}"
          aria-selected={tab === t.id}
          aria-controls="mine-panel"
          tabindex={tab === t.id ? 0 : -1}
          bind:this={tabs[t.id]}
          onclick={() => choose(t.id)}
          onkeydown={(event) => tabKey(event, index)}
          >{phone ? t.short : t.label}{#if counts[t.id] !== null}<span class="tabs__count">{counts[t.id]}</span>{/if}</button
        >
      {/each}
    </div>
  </div>
{/snippet}

{#if phone}
  <h1 class="vh">My runs</h1>
  <section class="card tabs window-fill member-runs" aria-labelledby="mine-title" data-fid="window">
    {@render strip()}
    {@render notice?.()}
    <div class="member-runs__panel member-runs__panel--phone" id="mine-panel" role="tabpanel" aria-labelledby="mine-tab-{tab}" tabindex="0">
      {#if tab === 'timings'}
        <TimingsPanel list={timings} {flow} {memberId} {phone} startDay={weeks.this?.days[0]?.dow} />
      {:else}
        {#if range}<p class="cap member-runs__range">{range}</p>{/if}
        {@render list()}
      {/if}
    </div>
  </section>
{:else}
  <div class="pageline" data-fid="page-line">
    <div class="pageline__head">
      <h1 class="pageline__title">My runs</h1>
      <p class="pageline__context">
        {#if week}· boss week <span class="mono">{range}</span> · <b class="mono">{owed.length}</b> answer{owed.length === 1 ? '' : 's'} owed{/if}
      </p>
    </div>
  </div>
  {@render notice?.()}
  <section class="card tabs window-fill member-runs" aria-labelledby="mine-title" data-fid="window">
    {@render strip()}
    <div class="member-runs__body">
      <div class="member-runs__panel" id="mine-panel" role="tabpanel" aria-labelledby="mine-tab-{tab}" tabindex="0">
        {#if tab === 'timings'}
          <TimingsPanel list={timings} {flow} {memberId} {phone} startDay={weeks.this?.days[0]?.dow} />
        {:else}
          {@render list()}
        {/if}
      </div>
      {#if tab !== 'timings'}
        <aside class="side-pane member-runs__aside" aria-label="Your runs" data-fid="myruns-aside">
          <p class="cap">{which === 'next' ? 'Next week' : 'This week'} · you</p>
          <dl class="member-runs__tiles">
            <div class="member-runs__tile">
              <dt class="cap">Runs</dt>
              <dd class="member-runs__value">{ahead.length} {which === 'next' ? 'planned' : 'left'}</dd>
              <dd class="member-runs__sub">of {runs.length} {which === 'next' ? 'next week' : 'this week'}</dd>
            </div>
            <div class="member-runs__tile">
              <dt class="cap">Answers owed</dt>
              <dd class="member-runs__value">{owed.length}</dd>
              <dd class="member-runs__sub">{owed[0] && week ? `${owed[0].bosses.map((b) => b.token).join(' + ')} · ${week.days[owed[0].day]?.dow ?? ''}` : 'none'}</dd>
            </div>
          </dl>
          <section class="account-sec" aria-labelledby="mine-calendar">
            <div class="account-sec__head">
              <h3 class="cap" id="mine-calendar">Calendar feed</h3>
              <span class="status-chip status-chip--warn account-sec__end">coming soon</span>
            </div>
            <p class="field__hint">Your runs in Google or Apple Calendar. A private link; revoke it any time in Account.</p>
            <button type="button" class="btn account-full" aria-disabled="true"><Icon name="copy" />Copy link</button>
          </section>
        </aside>
      {/if}
    </div>
  </section>
{/if}
<OwnerDialogs list={timings} {flow} {route} {session} {current} {phone} />
