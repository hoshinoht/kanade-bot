<!--
  The Rewrites log (persona rewrites of reminder headers and nudges) in the
  Extractions list-detail layout: one "Attempts" window with search and
  "Filters (n)" (model, kind, stage, verdict, dates) on its title bar, the
  attempts as a listbox beside the chosen one. Filters and the chosen
  attempt (`attempt`) are deep-linked through the query string. Phones show
  the list, then the attempt with "‹ Rewrites" in the top bar.
-->
<script lang="ts">
  import '@kanade/ui/styles/panes.scss';
  import '@kanade/ui/styles/extract.scss';
  import { LoadError, LoadingState, SINGLE_PANE_QUERY } from '@kanade/ui';
  import type { LogFacets, Rewrites } from '@kanade/api-types';
  import { tick, untrack } from 'svelte';
  import { activeCount, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import { PAGE_SIZE, paged } from '../pages/paging';
  import { pinnedFirst, Resource } from '../resource.svelte';
  import { getChrome } from '../shell/chrome';
  import PageLine from '../shell/PageLine.svelte';
  import type { AdminWeek } from '../store.svelte';
  import { attemptOf, withAttempt } from './format';
  import RewriteDetail from './RewriteDetail.svelte';
  import RewriteList from './RewriteList.svelte';

  let {
    store,
    search = '',
    onsearch,
    onselect,
  }: {
    store: AdminWeek;
    search?: string;
    onsearch?: (search: string) => void;
    /** Opens an attempt (`search` carries `attempt`); `open` pushes a history entry (phones: Back returns to the list). */
    onselect?: (search: string, open: boolean) => void;
  } = $props();
  const uid = $props.id();
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');
  const SCOPE = { chat: false, rewrites: true } as const;
  const searchOf = (filter: LogFilter) => toSearch(filter, SCOPE);

  const filter = $derived(parseFilter(search, SCOPE));
  const attempt = $derived(attemptOf(search));
  const rewrites = $derived(
    new Resource<Rewrites>(`/api/admin/rewrites${searchOf(filter)}`, { topics: ['rewrite'], keys: (data) => data.rows.map((row) => row.id) }),
  );
  $effect(() => rewrites.watch());
  let last = $state<Rewrites | null>(null);
  $effect(() => {
    if (rewrites.data) last = rewrites.data;
  });
  // A refused filter shows its error alone; `last` still feeds the filter facets.
  const view = $derived(rewrites.error ? null : last);
  // LogFilters' shape: verdicts are its outcomes.
  const facets = $derived<LogFacets | null>(last ? { models: last.facets.models, tools: [], outcomes: last.facets.verdicts, channels: [] } : null);
  const scopes = $derived(last ? { kinds: last.facets.kinds, stages: last.facets.stages } : { kinds: [], stages: [] });

  function apply(next: LogFilter) {
    onsearch?.(withAttempt(searchOf(next), attempt));
  }

  // svelte-ignore state_referenced_locally
  let query = $state(filter.q);
  $effect(() => {
    const text = query;
    if (text.trim() === filter.q.trim()) return;
    const timer = setTimeout(() => apply({ ...filter, q: text }), 250);
    return () => clearTimeout(timer);
  });
  // A q changed from outside the box (Clear, Back, a deep link) replaces what it shows.
  // svelte-ignore state_referenced_locally
  let seenQ = filter.q;
  $effect(() => {
    const q = filter.q;
    if (q === seenQ) return;
    seenQ = q;
    if (q.trim() !== query.trim()) query = q;
  });

  const rows = $derived(view?.rows ?? []);
  let page = $state(1);
  // Each new result set starts at the first page, or at the chosen attempt's page.
  let landed: Resource<Rewrites> | null = null;
  $effect(() => {
    const data = rewrites.data;
    if (!data || rewrites === landed) return;
    landed = rewrites;
    const at = data.rows.findIndex((r) => r.id === untrack(() => attempt));
    page = at >= 0 ? Math.floor(at / PAGE_SIZE) + 1 : 1;
  });
  const shown = $derived(paged(rows, page));
  const filtered = $derived(activeCount(filter) > 0);
  const accepted = $derived(rows.filter((row) => row.verdict === 'accepted').length);

  let phone = $state(false);
  $effect(() => {
    const media = window.matchMedia(SINGLE_PANE_QUERY);
    const update = () => (phone = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  // Wide: an attempt is always open (the first one on the page until one is chosen).
  const firstOpen = pinnedFirst(() => rewrites);
  const chosen = $derived(attempt || (phone ? '' : firstOpen(shown.rows, page)));

  const chrome = getChrome();
  const compact = $derived(phone && Boolean(attempt) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Rewrites', name: 'Back to the list (Rewrites)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  function pick(id: string, open: boolean) {
    const next = withAttempt(searchOf(filter), id);
    if (onselect) onselect(next, open && phone);
    else onsearch?.(next);
  }
  const pushedHere = () => (history.state as { rewriteDetail?: boolean } | null)?.rewriteDetail === true;
  function leaveDetail() {
    if (pushedHere()) history.back();
    else pick('', false);
  }

  // Phones swap list and attempt: focus follows into the attempt, and back to its row on return.
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detail = $state<{ focus: () => void }>();
  let was = untrack(() => attempt);
  $effect(() => {
    const now = attempt;
    const before = was;
    was = now;
    if (!phone || now === before) return;
    if (now && !before) void tick().then(() => detail?.focus());
    else if (!now && before) void tick().then(() => list?.focusOn(before));
  });
</script>

<PageLine title={view ? 'Rewrites' : ''} class={compact ? 'pageline--echo' : ''}>
  <h1>
    {#if view}{#if filtered}<span class="pageline__num">{rows.length}</span> of <span class="pageline__num">{view.total}</span>{:else}<span
          class="pageline__num">{view.total}</span
        >{/if} rewrites{:else}Rewrites{/if}
  </h1>
  {#snippet side()}
    {#if view && rows.length}<span class="chip chip--mono">{accepted} accepted</span>{/if}
  {/snippet}
</PageLine>

<section class="card extract-window window-fill" class:extract-window--compact={compact} aria-labelledby="{uid}-title">
  <div class="card__head extract-window__head">
    <h2 class="card__title" id="{uid}-title">Attempts</h2>
    <div class="extract-window__actions">
      <div class="extract-window__search" role="search">
        <label class="vh" for="{uid}-q">Search rewrites</label>
        <input id="{uid}-q" type="search" bind:value={query} placeholder="line, reply, code…" autocomplete="off" spellcheck="false" />
      </div>
      <div class="extract-window__filters">
        <LogFilters {filter} {facets} rewrites={scopes} members={store.members} week={store.week} onchange={apply} />
      </div>
    </div>
  </div>
  <div class="extract-window__body" class:extract-window__body--single={phone || !view || rows.length === 0}>
    <div class="extract-list" hidden={phone && Boolean(attempt)}>
      {#if rewrites.error}
        <LoadError thing="the rewrites" reason={rewrites.error} onretry={() => void rewrites.load()} level={3} />
      {:else if view}
        {#if rows.length === 0}
          <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
        {:else}
          <RewriteList bind:this={list} rows={shown.rows} selected={chosen} follow={!phone} timeZone={tz} onpick={pick} fresh={rewrites.fresh} />
          <Pager bind:page pages={shown.pages} total={rows.length} noun="attempt" />
        {/if}
      {:else}
        <LoadingState text="Loading rewrites…" />
      {/if}
    </div>
    {#if phone && attempt && !compact}
      <button type="button" class="btn extract-window__back" onclick={leaveDetail}>‹ All attempts</button>
    {/if}
    {#if chosen && (!phone || attempt)}
      <RewriteDetail bind:this={detail} id={chosen} timeZone={tz} />
    {/if}
  </div>
</section>
