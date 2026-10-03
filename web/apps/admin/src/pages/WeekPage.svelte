<script lang="ts" module></script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import type { Run, WeekKey } from '@kanade/api-types';
  import { dayNumber, experiments, flip, Icon, LoadingState, longDate, NapWindow, SPRING_BOUNCY, SPRING_BOUNCY_MS, runTitle, sortRuns } from '@kanade/ui';
  import Planner from '../planner/Planner.svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import type { Slot } from '../planner/keyboardMove';
  import { isPast, type AdminWeek } from '../store.svelte';
  import { directory, memberLabel } from '../names/directory.svelte';
  import Filters from '../week/Filters.svelte';
  import Glance from '../week/Glance.svelte';
  import RunsTable from '../week/RunsTable.svelte';
  import { activeFilters, applyFilter, filtering, NO_FILTER, type FilterKey, type WeekFilter } from '../week/filters';
  import { owedByMember } from '../week/waiting';

  export type WeekTab = 'planner' | 'runs' | 'answers';

  let {
    store,
    which,
    tab = $bindable(),
    onmove,
    onswap,
    onopen,
    onundo,
    onreread,
    busyChannels,
    selectedRun = null,
    pane,
  }: {
    store: AdminWeek;
    which: WeekKey;
    tab: WeekTab;
    onmove: (runId: string, to: Slot) => void;
    /** Exchange two runs' slots (a drop on a card, or S during a keyboard lift). */
    onswap: (runId: string, withId: string) => void;
    onopen: (runId: string) => void;
    onundo: () => void;
    onreread: (run: Run) => void;
    busyChannels: Set<string>;
    /** The run open in the side pane (wide screens) or the sheet. */
    selectedRun?: string | null;
    /** The run pane, rendered beside the board when a run is open on a wide screen (gate G4). */
    pane?: Snippet;
  } = $props();

  const uid = $props.id();
  const helpId = `${uid}-help`;
  const chrome = getChrome();
  const phone = $derived(chrome?.phone ?? false);
  // O5: the at-a-glance pane from 1200 px; below that the footer carries its facts.
  const roomy = new MediaQuery('(min-width: 1200px)', true);
  let filter = $state<WeekFilter>({ ...NO_FILTER });
  // v4: past (done) and cancelled runs are hidden until asked for.
  let showPast = $state(false);
  let helpOpen = $state(false);
  const matching = $derived(store.week ? applyFilter(store.week.runs, filter) : []);
  const hidden = $derived(showPast ? 0 : matching.filter(isPast).length);
  const shown = $derived(
    store.week ? { ...store.week, runs: showPast ? matching : matching.filter((r) => !isPast(r)) } : null,
  );
  const filtered = $derived(filtering(filter));
  const count = $derived(shown?.runs.length ?? 0);
  // The Answers view follows the filters: who still owes answers on the
  // matching runs (past ones too), and its per-day counts recomputed from them
  // the way the server counts (any answer is answered, none is waiting).
  const owed = $derived(owedByMember(sortRuns(matching), runTitle));
  const answerStats = $derived.by(() => {
    if (!store.stats || !filtered) return store.stats;
    const per_day = store.stats.per_day.map((d) => ({ day: d.day, answered: 0, waiting: 0 }));
    for (const run of matching) {
      const stat = per_day.find((d) => d.day === run.day);
      if (!stat) continue;
      for (const p of run.participants) {
        if (p.answer === 'waiting') stat.waiting += 1;
        else stat.answered += 1;
      }
    }
    return { per_day };
  });
  const waiting = $derived(store.stats ? store.stats.per_day.reduce((n, d) => n + d.waiting, 0) : null);
  // The channel filter offers only the shown week's party channels (not every
  // guild channel), named as the runs name them, else the directory, else the id.
  const partyChannels = $derived.by(() => {
    const out: { id: string; name: string }[] = [];
    for (const run of store.week?.runs ?? []) {
      if (!run.channel_id || out.some((c) => c.id === run.channel_id)) continue;
      out.push({ id: run.channel_id, name: run.channel || store.channels.find((c) => c.id === run.channel_id)?.name || run.channel_id });
    }
    return out.sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
  });
  // A channel with no run in the newly shown week would filter to nothing
  // behind a select that cannot show it: fall back to all channels.
  $effect(() => {
    if (store.week && filter.channel && !partyChannels.some((c) => c.id === filter.channel)) filter.channel = '';
  });
  const chips = $derived(
    activeFilters(
      filter,
      (id) => directory.label('channel', id, partyChannels.find((c) => c.id === id)?.name ?? ''),
      (id) => memberLabel(store.members, id),
    ),
  );
  const range = $derived.by(() => {
    const days = store.week?.days ?? [];
    const first = days[0];
    const last = days[days.length - 1];
    return first && last ? `${first.dow} ${dayNumber(first.date)} – ${last.dow} ${longDate(last.date)}` : '';
  });
  const glance = $derived(roomy.current && !phone && !selectedRun && tab === 'planner');
  const VIEWS: { id: WeekTab; label: string }[] = [
    { id: 'planner', label: 'Planner' },
    { id: 'runs', label: 'Runs' },
    { id: 'answers', label: 'Answers' },
  ];
  const viewTabs: Record<string, HTMLButtonElement> = {};

  // Own moves glide (FLIP): the store calls this just before the admin's own
  // change lands, so each card animates from where it was. Polls never do.
  $effect(() => {
    store.beforeChange = () => {
      if (tab !== 'planner') return;
      const bouncy = experiments.overshoot ? { easing: SPRING_BOUNCY, duration: SPRING_BOUNCY_MS } : {};
      void flip(document.querySelector('.board.planner'), { selector: '[data-run]', key: (el) => el.dataset.run, ...bouncy });
    };
    return () => (store.beforeChange = null);
  });

  function viewKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: VIEWS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = VIEWS[(target + VIEWS.length) % VIEWS.length]!;
    tab = next.id;
    viewTabs[next.id]?.focus();
  }

  function clearOne(key: FilterKey) {
    filter = { ...filter, [key]: '' };
  }

  // Still waiting → that member's runs.
  function showMember(id: string) {
    filter = { ...filter, member: id };
    tab = 'runs';
  }
</script>

{#snippet heading()}
  <h1>{#if shown}<span class="pageline__num">{count}</span> run{count === 1 ? '' : 's'}{filtered ? ', filtered' : ''}{:else}Week{/if}</h1>
{/snippet}

<!-- On phones the top bar names the page and the window's head row carries the count (B_PhoneWeek). -->
{#if !phone || !shown}
  <PageLine title={shown ? 'Week' : ''} class="week-line">
    {@render heading()}
    {#if range}<p class="pageline__context week-line__range mono">{range}</p>{/if}
  </PageLine>
{/if}

{#if shown}
  <!-- Gate G3: the week in one window, a supporting pane beside the board. -->
  <section class="card week-window window-fill" class:week-window--phone={phone} data-fid="window" aria-label="Week">
    <div class="card__head tabs__strip week-window__bar" data-fid="window-bar">
      <div class="tabs__tabs" role="tablist" aria-label="Week views" data-fid="window-tabs">
        {#each VIEWS as view, index (view.id)}
          <button
            type="button"
            role="tab"
            class="tabs__tab"
            id="{uid}-tab-{view.id}"
            aria-selected={tab === view.id}
            aria-controls="{uid}-panel"
            tabindex={tab === view.id ? 0 : -1}
            bind:this={viewTabs[view.id]}
            onclick={() => (tab = view.id)}
            onkeydown={(event) => viewKey(event, index)}
            >{view.label}{#if view.id === 'runs'}<span class="tabs__count">{count}</span>{:else if view.id === 'answers' && waiting !== null}<span
                class="tabs__count">{waiting}</span
              >{/if}</button
          >
        {/each}
      </div>
      <div class="week-window__actions" data-fid="week-actions">
        {#if !phone}
          <nav class="week-which" aria-label="Which week">
            <a href="/" aria-current={which === 'this' ? 'page' : undefined}>This week</a>
            <a href="/?week=next" aria-current={which === 'next' ? 'page' : undefined}>Next week</a>
          </nav>
        {/if}
        <Filters bind:filter channels={partyChannels} members={store.members} count={chips.length} icon={phone} />
        {#if !phone}
          {#if tab === 'planner'}
            <button
              type="button"
              class="btn week-window__icon"
              aria-label="How to move runs"
              title="How to move runs"
              aria-expanded={helpOpen}
              aria-controls={helpId}
              onclick={() => (helpOpen = !helpOpen)}><Icon name="info" /></button
            >
            <button
              type="button"
              class="btn week-window__icon"
              disabled={!store.lastMove}
              onclick={onundo}
              aria-keyshortcuts="Control+Z Meta+Z"
              aria-label={store.lastMove?.kind === 'swap' ? 'Undo swap' : 'Undo move'}
              title={store.lastMove?.kind === 'swap' ? 'Undo swap' : 'Undo move'}><Icon name="rotate-ccw" /></button
            >
          {/if}
          <button type="button" class="btn week-window__icon" onclick={() => store.refresh()} aria-label="Refresh" title="Refresh"><Icon name="refresh-cw" /></button>
        {/if}
      </div>
    </div>
    {#if phone}
      <div class="week-window__phonehead" data-fid="week-phone-head">
        <nav class="seg week-which week-which--phone" aria-label="Which week">
          <a href="/" aria-current={which === 'this' ? 'page' : undefined}>This week</a>
          <a href="/?week=next" aria-current={which === 'next' ? 'page' : undefined}>Next</a>
        </nav>
        <div class="week-window__count">{@render heading()}</div>
      </div>
    {/if}
    <div class="week-window__body">
      <!-- Always in the DOM: every movable card's aria-describedby points here. Planner-only. -->
      <p class="week-help" id={helpId} hidden={!helpOpen || tab !== 'planner'}>
        Drag a run to another day or between runs (on touch, press and hold first): it starts right after the run above, or ends right before
        the run below. Drop it on another run to swap their times. Or focus a run and press <kbd class="kbd">M</kbd>: arrow keys move it,
        <kbd class="kbd">Shift</kbd> with up puts it just after the run before, with down just before the run after, <kbd class="kbd">S</kbd> on
        another run's slot swaps, Enter drops it, Escape cancels. Its pane has a Move field for an exact time and Swap timing with….
      </p>
      <div
        class="week-surface week-surface--{tab}"
        role="tabpanel"
        id="{uid}-panel"
        aria-labelledby="{uid}-tab-{tab}"
        tabindex="0"
      >
        {#if tab === 'planner'}
          <Planner
            week={shown}
            {helpId}
            {selectedRun}
            {onmove}
            {onswap}
            onopen={(run: Run) => onopen(run.id)}
            onhold={(h) => (store.holding = h)}
            saving={store.mutating}
            {onreread}
            {busyChannels}
            step={store.runStep}
            allRuns={store.week?.runs}
          />
        {:else if tab === 'runs'}
          <RunsTable week={shown} selected={selectedRun} onopen={(run) => onopen(run.id)} />
        {:else}
          <!-- Loaded with its tab. -->
          {#await import('../week/AnswersView.svelte') then view}
            {#if answerStats}<view.default stats={answerStats} week={store.week!} {owed} onmember={showMember} />{/if}
          {:catch}
            <!-- After a deploy the old chunk name is gone; only a reload fetches the new one. -->
            <div class="empty" role="alert">
              <strong>The chart didn't load</strong>
              Kanade Admin has probably been updated since this page opened.
              <br /><button type="button" class="btn btn--primary" onclick={() => location.reload()}>Reload</button>
            </div>
          {/await}
        {/if}
      </div>
      {#if selectedRun && pane}
        {@render pane()}
      {:else if glance}
        <Glance summary={store.summary} week={store.week!} {onopen} />
      {/if}
    </div>
    <footer class="week-window__foot" data-fid="week-foot">
      {#if !phone}<span class="week-foot__runs"><b class="mono">{count}</b> run{count === 1 ? '' : 's'}{which === 'next' ? ' next week' : ''}</span>{/if}
      {#if hidden}
        <span class="week-foot__item"
          >{hidden} hidden · <button type="button" class="linklike" onclick={() => (showPast = true)}>Show {hidden > 1 ? 'them' : 'it'}</button></span
        >
      {:else if showPast}
        <span class="week-foot__item">Past shown · <button type="button" class="linklike" onclick={() => (showPast = false)}>Hide the past</button></span>
      {/if}
      {#each chips as chip (chip.key)}
        <button type="button" class="chip week-foot__chip" onclick={() => clearOne(chip.key)}
          >{chip.label}<span aria-hidden="true"> ×</span><span class="vh"> — remove</span></button
        >
      {/each}
      {#if !glance && store.summary}
        {#if store.summary.next}
          <button type="button" class="linklike week-foot__item week-foot__next" onclick={() => onopen(store.summary!.next!.run_id)}
            >Next <b class="mono">{store.summary.next.countdown}</b><span class="vh">: {store.summary.next.bosses}</span></button
          >
        {/if}
        {#if waiting !== null}<span class="week-foot__item" class:week-foot__item--warn={waiting > 0}><b class="mono">{waiting}</b> unanswered</span>{/if}
        {#if !phone}
          <a class="week-foot__item" class:week-foot__item--warn={store.summary.inbox > 0} href="/inbox">Inbox {store.summary.inbox}</a>
          <a class="week-foot__item" class:week-foot__item--warn={store.summary.model.busy} href="/limits">Model {store.summary.model.busy ? 'busy' : 'free'}</a>
        {/if}
      {/if}
      {#if !phone && store.week}
        <span class="week-foot__tz mono" title="Every time here is {store.week.timezone}; the boss week starts {store.week.reset}">{store.week.timezone}</span>
      {/if}
    </footer>
  </section>
{:else if store.fresh === 'loading'}
  <section class="card window-fill" aria-labelledby="loading-title">
    <div class="card__head"><h2 class="card__title" id="loading-title">Week</h2></div>
    <LoadingState text="Loading the week…" />
  </section>
{:else}
  <NapWindow title={store.fresh === 'offline' ? "You're offline" : "Kanade can't be reached"}>
    <p>
      {store.fresh === 'offline'
        ? 'Planning needs a connection. Nothing private is stored on this device.'
        : 'The week did not load. It will try again on its own.'}
    </p>
    {#snippet actions()}
      <button type="button" class="btn btn--primary" onclick={() => store.refresh()}>Try again</button>
    {/snippet}
  </NapWindow>
{/if}
