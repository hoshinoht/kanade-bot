<script lang="ts" module></script>

<script lang="ts">
  import type { Run, WeekKey } from '@kanade/api-types';
  import { Icon, NapWindow, RunTable } from '@kanade/ui';
  import Planner from '../planner/Planner.svelte';
  import PageLine from '../shell/PageLine.svelte';
  import type { Slot } from '../planner/keyboardMove';
  import type { AdminWeek } from '../store.svelte';
  import Filters from '../week/Filters.svelte';
  import NowTiles from '../week/NowTiles.svelte';
  import { applyFilter, filtering, NO_FILTER, type WeekFilter } from '../week/filters';

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
  } = $props();

  const uid = $props.id();
  const helpId = `${uid}-help`;
  let filter = $state<WeekFilter>({ ...NO_FILTER });
  // v4: past (done) and cancelled runs are hidden until asked for.
  let showPast = $state(false);
  let helpOpen = $state(false);
  const PAST = ['done', 'cancelled'];
  const matching = $derived(store.week ? applyFilter(store.week.runs, filter) : []);
  const hidden = $derived(showPast ? 0 : matching.filter((r) => PAST.includes(r.status)).length);
  const shown = $derived(
    store.week ? { ...store.week, runs: showPast ? matching : matching.filter((r) => !PAST.includes(r.status)) } : null,
  );
  const filtered = $derived(filtering(filter));
  const count = $derived(shown?.runs.length ?? 0);
  const VIEWS: { id: WeekTab; label: string }[] = [
    { id: 'planner', label: 'Planner' },
    { id: 'runs', label: 'Runs' },
    { id: 'answers', label: 'Answers' },
  ];
  const viewTabs: Record<string, HTMLButtonElement> = {};

  // Area follows importance: on phones and short frames the filter card folds
  // into a "Filters (n)" button in this header (docs/v5/pwa-design-guidelines.md).
  let compact = $state(false);
  // Phone landscape: the summary line joins the header row instead of taking its own.
  let landscape = $state(false);
  $effect(() => {
    const query = window.matchMedia('(max-height: 500px)');
    const update = () => (landscape = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  $effect(() => {
    const query = window.matchMedia('(max-width: 899px), (max-height: 700px)');
    const update = () => (compact = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
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
</script>

<!-- v4 week.html's header row as the page line, carrying v5's view switch and
  help: the board sits directly under the filters with no window chrome around it. -->
<PageLine title={shown ? 'Week' : ''} class="week-head">
  <h1>{shown ? `${count} run${count === 1 ? '' : 's'}${filtered ? ', filtered' : ''}` : 'Week'}</h1>
  {#if hidden}
    <p class="note week-head__past">
      {hidden} hidden ·
      <button type="button" class="linklike" onclick={() => (showPast = true)}>Show {hidden > 1 ? 'them' : 'it'}</button>
    </p>
  {:else if showPast}
    <p class="note week-head__past">
      Past shown · <button type="button" class="linklike" onclick={() => (showPast = false)}>Hide the past</button>
    </p>
  {/if}
  {#snippet side()}
    <nav class="seg week-head__week" aria-label="Which week">
      <a href="/" aria-current={which === 'this' ? 'page' : undefined}>This <span class="week-head__wk">week</span></a>
      <a href="/?week=next" aria-current={which === 'next' ? 'page' : undefined}>Next <span class="week-head__wk">week</span></a>
    </nav>
    <div class="seg week-head__views" role="tablist" aria-label="Week views">
      {#each VIEWS as view, index (view.id)}
        <button
          type="button"
          role="tab"
          id="{uid}-tab-{view.id}"
          aria-selected={tab === view.id}
          aria-controls="{uid}-panel"
          tabindex={tab === view.id ? 0 : -1}
          bind:this={viewTabs[view.id]}
          onclick={() => (tab = view.id)}
          onkeydown={(event) => viewKey(event, index)}
          >{view.label}{#if view.id === 'runs' && shown}<span class="seg__count">{count}</span>{/if}</button
        >
      {/each}
    </div>
    {#if tab === 'planner'}
      <button
        type="button"
        class="btn btn--ghost week-head__help"
        title="How to move runs"
        aria-expanded={helpOpen}
        aria-controls={helpId}
        onclick={() => (helpOpen = !helpOpen)}
      >
        <Icon name="info" /> <span class="week-head__label">How to move runs</span>
      </button>
    {/if}
    {#if landscape && store.summary}<NowTiles summary={store.summary} {onopen} inline />{/if}
    {#if compact}<Filters bind:filter channels={store.channels} members={store.members} compact />{/if}
    <div class="page-head__side">
      <button type="button" class="btn" disabled={!store.lastMove} onclick={onundo} aria-keyshortcuts="Control+Z Meta+Z" title={store.lastMove?.kind === 'swap' ? 'Undo swap' : 'Undo move'}>
        <Icon name="rotate-ccw" /> <span class="week-head__label">{store.lastMove?.kind === 'swap' ? 'Undo swap' : 'Undo move'}</span>
      </button>
      <button type="button" class="btn" onclick={() => store.refresh()} title="Refresh"
        ><Icon name="refresh-cw" /> <span class="week-head__label">Refresh</span></button
      >
    </div>
    <!-- Always in the DOM: every movable card's aria-describedby points here. -->
    <!-- Planner-only help: its toggle leaves with the Planner, so the text does too. -->
    <p class="week-head__helptext" id={helpId} hidden={!helpOpen || tab !== 'planner'}>
      Drag a run to another day or between runs (on touch, press and hold first): it starts right after the run above, or ends right before
      the run below. Or focus a run and press <kbd class="kbd">M</kbd>: arrow keys move it, <kbd class="kbd">Shift</kbd> with up puts it just
      after the run before, with down just before the run after, Enter drops it, Escape cancels. Its sheet has a Move field for an exact time.
    </p>
  {/snippet}
</PageLine>

{#if store.summary && !landscape}<NowTiles summary={store.summary} {onopen} />{/if}
{#if !compact}<Filters bind:filter channels={store.channels} members={store.members} />{/if}

{#if shown}
  <div
    class="week-surface"
    class:week-surface--sheet={tab !== 'planner'}
    role="tabpanel"
    id="{uid}-panel"
    aria-labelledby="{uid}-tab-{tab}"
    tabindex="0"
  >
    {#if tab === 'planner'}
      <Planner
        week={shown}
        {helpId}
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
      <RunTable week={shown} onopen={(run) => onopen(run.id)} />
    {:else}
      <!-- uPlot loads with its view, keeping it out of the initial bundle. -->
      {#await import('../AnswersChart.svelte') then chart}
        {#if store.stats}<chart.default stats={store.stats} week={store.week!} />{/if}
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
{:else if store.fresh === 'loading'}
  <section class="card window-fill" aria-busy="true" aria-labelledby="loading-title">
    <div class="card__head"><h2 class="card__title" id="loading-title">Loading the week…</h2></div>
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
