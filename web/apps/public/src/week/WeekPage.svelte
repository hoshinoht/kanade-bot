<!--
  The member's Week (boards Main, Week-RunMine, Week-RunOther, WeekList,
  WeekList-Selected, ConfirmAnswer, States*; phone PhoneWeek, PhoneRun*,
  PhoneStates*): the admin Week window (`pages/WeekPage.svelte`,
  `_week.scss`). Title-bar tabs Week / List n; This week / Next week, Only
  my runs and Refresh; the board beside Your week and the footer, the list
  alone at full width, or either with the open run's pane (`?run=`,
  `?view=list`, `?week=next` deep-link it). Phones show the week as one
  list; an open run is its own screen (PhoneRun). The member answers and
  moves their own runs here (`RunFlow`); back from "Confirm it's you"
  (`&answer=`) the run is open with that answer picked, not saved yet.
  Times are the guild's, named in the footer.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek, PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { dayNumber, enter, flip, Icon, LoadingState, longDate, Presence, replay, StateNote, WavyProgress, weekProgress, type Toaster } from '@kanade/ui';
  import { tick, untrack, type Snippet } from 'svelte';
  import { countdownWords, isPast, nextOwn, shownRuns } from '../member';
  import type { Route } from '../route.svelte';
  import type { MemberWeeks, WeekKey } from '../weeks.svelte';
  import ConfirmRun from '../writes/ConfirmRun.svelte';
  import { RunFlow } from '../writes/flow.svelte';
  import type { RunWrites } from '../writes/runWrites.svelte';
  import { returnedChoice, type Choice } from '../writes/runs';
  import MemberCard from './MemberCard.svelte';
  import PhoneRun from './PhoneRun.svelte';
  import RunPane from './RunPane.svelte';
  import WeekBoard from './WeekBoard.svelte';
  import WeekList from './WeekList.svelte';
  import YourWeek from './YourWeek.svelte';

  let {
    weeks,
    runs: writes,
    route,
    session,
    current,
    phone,
    toaster,
    notice,
  }: {
    weeks: MemberWeeks;
    /** The member's answers and moves (`Portal.runs`). */
    runs: RunWrites;
    route: Route;
    session: PublicSession;
    /** This device's row in the session list, for when it signed in (Confirm it's you). */
    current: PublicSessionRow | null;
    phone: boolean;
    toaster: Toaster;
    /** The offline notice: under the page line, or inside the phone window. */
    notice?: Snippet;
  } = $props();

  const memberId = $derived(session.member.id);
  const flow = untrack(() => new RunFlow(writes, toaster));
  const which: WeekKey = $derived(route.params.get('week') === 'next' ? 'next' : 'this');
  const view = $derived(route.params.get('view') === 'list' ? 'list' : 'week');
  const week = $derived(weeks.week(which));
  let onlyMine = $state(false);
  let showPast = $state(false);
  const runs = $derived(week ? shownRuns(week.runs, { onlyMine, showPast }) : []);
  const hidden = $derived(week && !showPast ? week.runs.filter((r) => isPast(r) && (!onlyMine || r.mine)).length : 0);

  // Back from the fresh sign-in (`&answer=`): that answer picked on the open run until a press.
  let picked = $state<{ run: string; answer: Choice } | null>(null);
  $effect(() => {
    const answer = returnedChoice(route.params.get('answer'));
    if (!route.params.has('answer')) return;
    const run = route.params.get('run') ?? '';
    untrack(() => {
      picked = answer && run ? { run, answer } : null;
      route.set({ answer: '' });
    });
  });
  const yoursCount = $derived(week ? week.runs.filter((r) => r.mine).length : 0);
  // The open run stays open even when a filter hides its card.
  const selected = $derived(week?.runs.find((r) => r.id === route.params.get('run')) ?? null);
  const range = $derived.by(() => {
    const first = week?.days[0];
    const last = week?.days[week.days.length - 1];
    return first && last ? `${first.dow} ${dayNumber(first.date)} – ${last.dow} ${longDate(last.date)}` : '';
  });
  const shortRange = $derived(range.replace(/ \w{3}$/, ''));
  const mine = $derived(week ? nextOwn(week) : null);
  const weekBar = $derived(week ? weekProgress(week) : null);
  const uid = $props.id();
  const VIEWS = [
    { id: 'week', label: 'Week' },
    { id: 'list', label: 'List' },
  ] as const;
  const viewTabs: Record<string, HTMLButtonElement> = {};

  // The pane outlives the selection by its exit (`is-leaving`), as the admin's side panes do.
  const pane = new Presence<{ run: MemberRun; week: MemberWeek }>();
  $effect(() => pane.set(!phone && week && selected ? { run: selected, week } : null));
  // While open the pane reads the live run (the presence copy lags an effect behind).
  const paneRun = $derived(!phone && week && selected ? { run: selected, week } : pane.shown);

  // A week from elsewhere (a timed read) glides the cards it moved and marks
  // the runs it brings in, as the admin board does; the member's own Refresh never does.
  $effect(() => {
    weeks.beforeArrival = (added) => {
      void flip(document.querySelector('.member-board, .member-phone__list'), { selector: '[data-run]', key: (el) => el.dataset.run });
      if (!added.length) return;
      void tick().then(() => {
        for (const id of added) for (const el of document.querySelectorAll(`[data-run="${CSS.escape(id)}"]`)) replay(el, 'data-new');
      });
    };
    return () => (weeks.beforeArrival = null);
  });

  // Phones swap the week and the open run: the run comes in forward
  // (PhoneRun), and the week comes back backward when it closes.
  let returns = $state(0);
  let runWasOpen = false;
  $effect(() => {
    const open = phone && selected !== null;
    if (runWasOpen && !open && phone) untrack(() => returns++);
    runWasOpen = open;
  });

  function open(run: MemberRun, to: WeekKey = which) {
    if (run.id === selected?.id && to === which) {
      close();
      return;
    }
    route.set({ week: to === 'next' ? 'next' : '', run: run.id });
  }

  async function close() {
    const id = selected?.id;
    route.set({ run: '' });
    // Back to the card or row that opened it.
    await tick();
    if (id) document.querySelector<HTMLElement>(`[data-run="${CSS.escape(id)}"], [data-row="${CSS.escape(id)}"] .week-runs__open`)?.focus();
  }

  function pickWeek(event: MouseEvent, to: WeekKey) {
    event.preventDefault();
    route.set({ week: to === 'next' ? 'next' : '', run: '' });
  }

  function viewKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: VIEWS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = VIEWS[(target + VIEWS.length) % VIEWS.length]!;
    route.set({ view: next.id === 'list' ? 'list' : '' });
    viewTabs[next.id]?.focus();
  }

  async function refresh() {
    await weeks.refresh();
    // Offline has its own notice; anything else the server refused is said once.
    if (weeks.error && !weeks.offline) toaster.show({ message: `Couldn't refresh the week: ${weeks.error}`, tone: 'error', action: { label: 'Try again', run: () => void refresh() } });
  }
</script>

{#snippet heading()}
  {#if week}<b class="mono">{runs.length}</b> run{runs.length === 1 ? '' : 's'} · <span class="mono">{range}</span> · you're in <b class="mono">{yoursCount}</b
    >{/if}
{/snippet}

{#snippet whichWeek(phoneSeg: boolean)}
  <nav class="week-which" class:seg={phoneSeg} class:week-which--phone={phoneSeg} aria-label="Which week">
    <a href="/" aria-current={which === 'this' ? 'page' : undefined} onclick={(event) => pickWeek(event, 'this')}>This week</a>
    <a href="/?week=next" aria-current={which === 'next' ? 'page' : undefined} onclick={(event) => pickWeek(event, 'next')}>{phoneSeg ? 'Next' : 'Next week'}</a>
  </nav>
{/snippet}

{#snippet onlyMineButton(label: string)}
  <button type="button" class="btn member-toggle" aria-pressed={onlyMine} aria-label={phone ? 'Only my runs' : undefined} onclick={() => (onlyMine = !onlyMine)}
    ><Icon name="users" />{label}</button
  >
{/snippet}

{#snippet refreshButton()}
  <button type="button" class="btn week-window__icon" aria-label="Refresh the week" title="Refresh the week" onclick={() => void refresh()}><Icon name="refresh-cw" /></button>
{/snippet}

{#snippet foot()}
  <footer class="week-window__foot" data-fid="week-foot">
    <span class="week-foot__runs"><b class="mono">{runs.length}</b> run{runs.length === 1 ? '' : 's'}{which === 'next' ? ' next week' : ''}</span>
    {#if hidden}
      <span class="week-foot__item">{hidden} done hidden · <button type="button" class="linklike" onclick={() => (showPast = true)}>Show {hidden > 1 ? 'them' : 'it'}</button></span>
    {:else if showPast}
      <span class="week-foot__item">Past shown · <button type="button" class="linklike" onclick={() => (showPast = false)}>Hide the past</button></span>
    {/if}
    {#if !phone}
      {#if mine && week && countdownWords(mine, week)}
        <button type="button" class="linklike week-foot__item week-foot__next" onclick={() => open(mine)}>Your next <b class="mono">{countdownWords(mine, week)}</b></button>
      {/if}
      {#if weekBar}
        <span class="week-foot__item week-foot__week" title={weekBar.text}
          ><span>{weekBar.text}</span><WavyProgress class="wavy--inline week-foot__bar" value={weekBar.value} max={weekBar.max} wavy={false} label="Boss week" text={weekBar.text} /></span
        >
      {/if}
      {#if week}<span class="week-foot__tz mono" title="Every time here is {week.timezone}; the boss week starts {week.reset}">{week.timezone}</span>{/if}
    {/if}
  </footer>
{/snippet}

{#snippet skeleton()}
  <!-- The week's shape while its first read is on the way (board States). -->
  <div class="board member-board member-board--loading" data-fid="week-board" aria-busy="true">
    {#each { length: 7 }, n (n)}
      <section class="board__col member-skel" aria-hidden="true" data-fid="week-day">
        <div class="board__head" data-fid="week-day-head"><span class="member-skel__bar"></span></div>
        <div class="board__runs"><span class="member-skel__card"></span></div>
      </section>
    {/each}
    <div class="member-board__loading"><LoadingState text="Loading the week…" /></div>
  </div>
{/snippet}

{#if phone && selected && week}
  <PhoneRun
    run={selected}
    {week}
    {which}
    {memberId}
    {flow}
    {route}
    picked={picked?.run === selected.id ? picked.answer : null}
    onpress={() => (picked = null)}
  />
{:else if !week && weeks.error && !weeks.offline}
  <section class="card window-fill" aria-labelledby="{uid}-failed">
    <div class="card__head"><h1 class="card__title" id="{uid}-failed">Week</h1></div>
    <StateNote icon="alert-circle" level={2} title="The week didn't load">
      {weeks.error}
      {#snippet actions()}
        <button type="button" class="btn btn--primary" onclick={() => void weeks.refresh()}>Try again</button>
      {/snippet}
    </StateNote>
  </section>
{:else if phone}
  <h1 class="vh">Week</h1>
  <section class="card window-fill week-window week-window--phone member-week" aria-label="Week" data-fid="window" {@attach enter(returns || null, 'backward')}>
    <div class="card__head week-window__bar member-week__phonebar" data-fid="window-bar">
      <span class="card__title mono member-week__range">{shortRange}</span>
      <div class="week-window__actions">{@render onlyMineButton('Mine')}{@render refreshButton()}</div>
    </div>
    <div class="week-window__phonehead" data-fid="week-phone-head">
      {@render whichWeek(true)}
      <p class="week-window__count">{#if week}{runs.length} run{runs.length === 1 ? '' : 's'} · you're in {yoursCount}{/if}</p>
    </div>
    {@render notice?.()}
    <div class="week-window__body">
      <div class="week-surface member-phone" tabindex="-1">
        {#if week}
          <div class="member-phone__list" data-fid="week-board">
            {#each week.days as day (day.index)}
              {@const today = runs.filter((r) => r.day === day.index)}
              {#if today.length}
                <h2 class="member-phone__day" class:member-phone__day--today={day.is_today} data-fid="week-day-head">
                  <span>{day.dow} {dayNumber(day.date)}{day.is_today ? ' · today' : ''}</span>
                  <span class="mono member-phone__count">{today.length} run{today.length === 1 ? '' : 's'}</span>
                </h2>
                {#each today as run (run.id)}
                  <MemberCard {run} {week} {memberId} phone onopen={(r) => open(r)} data-run={run.id} data-fid="week-card" />
                {/each}
              {/if}
            {/each}
            {#if !runs.length}
              <p class="note">{onlyMine ? "You're in no runs" : 'No runs'}{which === 'next' ? ' next week' : ' this week'}.</p>
            {/if}
          </div>
        {:else}
          <div class="member-phone__list member-phone__list--loading" data-fid="week-board" aria-busy="true"><LoadingState text="Loading the week…" /></div>
        {/if}
      </div>
    </div>
    {@render foot()}
  </section>
{:else}
  <div class="pageline" data-fid="page-line">
    <div class="pageline__head">
      <h1 class="pageline__title">Week</h1>
      <p class="pageline__context">{@render heading()}</p>
    </div>
  </div>
  {@render notice?.()}
  <section class="card tabs window-fill week-window member-week" aria-label="Week" data-fid="window">
    <div class="card__head tabs__strip week-window__bar" data-fid="window-bar">
      <div class="tabs__tabs" role="tablist" aria-label="Week views" data-fid="window-tabs">
        {#each VIEWS as v, index (v.id)}
          <button
            type="button"
            role="tab"
            class="tabs__tab"
            id="{uid}-tab-{v.id}"
            aria-selected={view === v.id}
            aria-controls="{uid}-panel"
            tabindex={view === v.id ? 0 : -1}
            bind:this={viewTabs[v.id]}
            onclick={() => route.set({ view: v.id === 'list' ? 'list' : '' })}
            onkeydown={(event) => viewKey(event, index)}
            >{v.label}{#if v.id === 'list' && week}<span class="tabs__count">{runs.length}</span>{/if}</button
          >
        {/each}
      </div>
      <div class="week-window__actions" data-fid="window-filters">
        {@render whichWeek(false)}
        {@render onlyMineButton('Only my runs')}
        {@render refreshButton()}
      </div>
    </div>
    <div class="week-window__body">
      <div class="week-surface week-surface--{view === 'list' ? 'runs' : 'planner'}" role="tabpanel" id="{uid}-panel" aria-labelledby="{uid}-tab-{view}" tabindex="0">
        {#if !week}
          {@render skeleton()}
        {:else if view === 'list'}
          <WeekList {week} {runs} {memberId} selected={selected?.id ?? null} onopen={(r) => open(r)} />
        {:else}
          <WeekBoard {week} {runs} {memberId} selected={selected?.id ?? null} onopen={(r) => open(r)} />
        {/if}
      </div>
      <!-- The list runs the window's full width with no Your week or footer (boards WeekList*): a row opens the pane. -->
      {#if paneRun}
        {#key paneRun.run.id}<RunPane
            run={paneRun.run}
            week={paneRun.week}
            {which}
            {memberId}
            {flow}
            {route}
            picked={picked?.run === paneRun.run.id ? picked.answer : null}
            onpress={() => (picked = null)}
            leaving={pane.leaving && !selected}
            onleft={(event) => pane.done(event)}
            onclose={() => void close()}
          />{/key}
      {:else if week && weeks.this && view !== 'list'}
        <YourWeek current={weeks.this} next={weeks.next} {memberId} arrival={weeks.arrival} {flow} {route} onopen={open} />
      {:else if !week}
        <aside class="side-pane week-glance member-glance" aria-label="Your week" data-fid="glance"></aside>
      {/if}
    </div>
    {#if view !== 'list' || !week}{@render foot()}{/if}
  </section>
{/if}
<ConfirmRun {flow} {session} {current} {phone} />
