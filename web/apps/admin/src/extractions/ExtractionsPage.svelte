<!--
  v4 extractions.html as list-detail (M3E B_Extract, gate G5): one "Calls"
  window with search, "Filters (n)" and "Re-read channels" on its title bar,
  the calls as a listbox beside the chosen call. Server-side filters (model,
  dates, outcome, channel, member, text) and the chosen call (`call`) are
  deep-linked through the query string. Phones show the list, then the call
  with "‹ Extractions" in the top bar — one scrolling panel either way.
-->
<script lang="ts">
  import '@kanade/ui/styles/panes.scss';
  import '@kanade/ui/styles/extract.scss';
  import { Icon, LoadError, LoadingState, SINGLE_PANE_QUERY } from '@kanade/ui';
  import type { Channel, Extractions } from '@kanade/api-types';
  import { tick, untrack } from 'svelte';
  import { activeCount, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import { PAGE_SIZE, paged } from '../pages/paging';
  import { pinnedFirst, Resource } from '../resource.svelte';
  import { getChrome } from '../shell/chrome';
  import PageLine from '../shell/PageLine.svelte';
  import type { AdminWeek } from '../store.svelte';
  import CallList from './CallList.svelte';
  import { callOf, withCall } from './code';
  import ExtractionDetail from './ExtractionDetail.svelte';
  import RescanPanel from './RescanPanel.svelte';

  let {
    store,
    search = '',
    onsearch,
    onselect,
  }: {
    store: AdminWeek;
    search?: string;
    onsearch?: (search: string) => void;
    /** Opens a call (`search` carries `call`); `open` pushes a history entry (phones: Back returns to the list). */
    onselect?: (search: string, open: boolean) => void;
  } = $props();
  const uid = $props.id();
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');

  const filter = $derived(parseFilter(search, { chat: false }));
  const call = $derived(callOf(search));
  // A pasted Chat link's tool/latency keys leave the URL rather than linger as dead chips.
  $effect(() => {
    const clean = toSearch(filter);
    if (clean !== toSearch(parseFilter(search))) onsearch?.(withCall(clean, call));
  });
  const extractions = $derived(new Resource<Extractions>(`/api/admin/extractions${toSearch(filter)}`, { topics: ['extraction'] }));
  const targets = new Resource<Channel[]>('/api/admin/rescan/targets');
  $effect(() => extractions.watch());
  $effect(() => void targets.load());
  let last = $state<Extractions | null>(null);
  $effect(() => {
    if (extractions.data) last = extractions.data;
  });
  // A refused filter shows its error alone; `last` still feeds the filter facets.
  const view = $derived(extractions.error ? null : last);

  function apply(next: LogFilter) {
    onsearch?.(withCall(toSearch(next), call));
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
  // Each new result set starts at the first page, or at the chosen call's page
  // (a deep link); a live refresh of the same set keeps the page.
  let landed: Resource<Extractions> | null = null;
  $effect(() => {
    const data = extractions.data;
    if (!data || extractions === landed) return;
    landed = extractions;
    const at = data.rows.findIndex((r) => r.id === untrack(() => call));
    page = at >= 0 ? Math.floor(at / PAGE_SIZE) + 1 : 1;
  });
  const shown = $derived(paged(rows, page));
  const filtered = $derived(activeCount(filter) > 0);

  let phone = $state(false);
  $effect(() => {
    const media = window.matchMedia(SINGLE_PANE_QUERY);
    const update = () => (phone = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  // Wide: a call is always open (the first one on the page until one is chosen).
  // That default stays put when a refresh adds calls above it.
  const firstOpen = pinnedFirst(() => extractions);
  const chosen = $derived(call || (phone ? '' : firstOpen(shown.rows, page)));

  const chrome = getChrome();
  const compact = $derived(phone && Boolean(call) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Extractions', name: 'Back to the list (Extractions)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  function pick(id: string, open: boolean) {
    const next = withCall(toSearch(filter), id);
    if (onselect) onselect(next, open && phone);
    else onsearch?.(next);
  }
  // Whether the open call's entry came from a pick here (so leaving pops it) or a deep link.
  const pushedHere = () => (history.state as { extractDetail?: boolean } | null)?.extractDetail === true;
  function leaveDetail() {
    if (pushedHere()) history.back();
    else pick('', false);
  }

  // Phones swap list and call: focus follows into the call, and back to its row on return (Back included).
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detail = $state<{ focus: () => void }>();
  let was = untrack(() => call);
  $effect(() => {
    const now = call;
    const before = was;
    was = now;
    if (!phone || now === before) return;
    if (now && !before) void tick().then(() => detail?.focus());
    else if (!now && before) void tick().then(() => list?.focusOn(before));
  });

  // Re-read: a title-bar popover that stays mounted, so a running job keeps its card and progress.
  let rereadOpen = $state(false);
  // Mounted on first open, then kept: the panel's cards are not the window's until asked for.
  let rereadMounted = $state(false);
  let rereadButton = $state<HTMLButtonElement>();
  let rereadPanel = $state<HTMLDivElement>();
  let rescan = $state<{ choose: (ids: string[]) => void }>();
  // While extraction is off the server refuses a re-read: say so before anyone presses it.
  const rescanOff = $derived(store.summary?.rescan_off ?? null);
  async function openReread(channel?: string) {
    if (rescanOff) return;
    rereadMounted = true;
    rereadOpen = true;
    await tick();
    if (channel) rescan?.choose([channel]);
    rereadPanel?.querySelector<HTMLElement>(channel ? 'button[type="submit"]' : 'input')?.focus({ preventScroll: true });
  }
  function closeReread(refocus: boolean) {
    rereadOpen = false;
    if (refocus) rereadButton?.focus({ preventScroll: true });
  }
  $effect(() => {
    if (!rereadOpen) return;
    const away = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!rereadPanel?.contains(target) && !rereadButton?.contains(target)) closeReread(false);
    };
    document.addEventListener('pointerdown', away, true);
    return () => document.removeEventListener('pointerdown', away, true);
  });
  const canReread = (channel: string) => !rescanOff && Boolean(channel) && (targets.data ?? []).some((t) => t.id === channel);
</script>

<PageLine title={view ? 'Extractions' : ''} class={compact ? 'pageline--echo' : ''}>
  <h1>{#if view}{#if filtered}<span class="pageline__num">{rows.length}</span> of <span class="pageline__num">{view.total}</span>{:else}<span class="pageline__num">{view.total}</span>{/if} model calls{:else}Extractions{/if}</h1>
  {#snippet side()}
    {#if view}<span class="chip chip--mono extract-model">{view.model}</span>{/if}
  {/snippet}
</PageLine>

<section class="card extract-window window-fill" class:extract-window--compact={compact} data-fid="window" aria-labelledby="{uid}-title">
  <div class="card__head extract-window__head" data-fid="window-bar">
    <h2 class="card__title" id="{uid}-title">Calls</h2>
    <div class="extract-window__actions" data-fid="extract-actions">
      <div class="extract-window__search" data-fid="window-search" role="search">
        <label class="vh" for="{uid}-q">Search calls</label>
        <input id="{uid}-q" type="search" bind:value={query} placeholder="message, id…" autocomplete="off" spellcheck="false" />
      </div>
      <div class="extract-window__filters" data-fid="extract-filters">
        <LogFilters {filter} facets={last?.facets ?? null} members={store.members} week={store.week} onchange={apply} />
      </div>
      <button
        type="button"
        class="btn extract-window__reread"
        data-fid="extract-reread"
        aria-expanded={rereadOpen}
        aria-controls="{uid}-reread"
        aria-disabled={rescanOff !== null && !rereadOpen}
        aria-describedby={rescanOff ? `${uid}-reread-off` : undefined}
        bind:this={rereadButton}
        onclick={() => (rereadOpen ? closeReread(false) : void openReread())}><Icon name="refresh-cw" /><span>Re-read channels</span></button
      >
    </div>
  </div>
  {#if rescanOff}<p class="extract-window__off" id="{uid}-reread-off">{rescanOff}</p>{/if}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="extract-reread"
    id="{uid}-reread"
    role="group"
    aria-label="Re-read the party channels"
    hidden={!rereadOpen}
    bind:this={rereadPanel}
    onkeydown={(event) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        closeReread(true);
      }
    }}
  >
    {#if rereadMounted}<RescanPanel targets={targets.data ?? []} off={rescanOff} bind:this={rescan} />{/if}
  </div>
  <div class="extract-window__body" class:extract-window__body--single={phone || !view || rows.length === 0}>
    <div class="extract-list" data-fid="extract-list" hidden={phone && Boolean(call)}>
      {#if extractions.error}
        <LoadError thing="the calls" reason={extractions.error} onretry={() => void extractions.load()} level={3} />
      {:else if view}
        {#if rows.length === 0}
          <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
        {:else}
          <CallList bind:this={list} rows={shown.rows} selected={chosen} follow={!phone} timeZone={tz} onpick={pick} />
          <Pager bind:page pages={shown.pages} total={rows.length} noun="call" />
        {/if}
      {:else}
        <LoadingState text="Loading calls…" />
      {/if}
    </div>
    {#if phone && call && !compact}
      <button type="button" class="btn extract-window__back" onclick={leaveDetail}>‹ All calls</button>
    {/if}
    {#if chosen && (!phone || call)}
      <ExtractionDetail bind:this={detail} id={chosen} timeZone={tz} {canReread} {rescanOff} onreread={(channel) => void openReread(channel)} />
    {/if}
  </div>
</section>
