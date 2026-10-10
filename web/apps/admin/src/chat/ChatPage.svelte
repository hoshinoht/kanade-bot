<!--
  Chat (M3E board B_Chat, gate G5): one "Interactions" window, the list of
  turns beside the open turn. `/chat/:id` deep-links a turn; wide screens show
  the first row when none is named. Server-side filters (model, dates,
  outcome, channel, member, text, tool, minimum latency) ride in the query
  string. Phones show the list, then the turn with a back step.
-->
<script lang="ts">
  import '@kanade/ui/styles/panes.scss';
  // Its global `pre` (prompts, raw replies, traces) styles the turn's code text.
  import '@kanade/ui/styles/evidence.scss';
  import '@kanade/ui/styles/chat.scss';
  import { LoadError, LoadingState, SINGLE_PANE_QUERY, enter, type Toaster } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import ModelStats from './ModelStats.svelte';
  import ChatList from './ChatList.svelte';
  import ChatTurn from './ChatTurn.svelte';
  import type { AdminWeek } from '../store.svelte';
  import type { Chat } from '@kanade/api-types';
  import { activeCount, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import { pinnedFirst, Resource } from '../resource.svelte';

  let {
    store,
    toaster,
    id = '',
    search = '',
    onsearch,
    onselect,
  }: {
    store: AdminWeek;
    toaster?: Toaster;
    /** The open turn (`/chat/:id`); empty on `/chat`. */
    id?: string;
    search?: string;
    onsearch?: (search: string) => void;
    /** Opens a turn (empty: the list); `open` pushes a history entry (phones: Back returns to the list). */
    onselect?: (id: string, open: boolean) => void;
  } = $props();
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');

  const filter = $derived(parseFilter(search));
  const chat = $derived(new Resource<Chat>(`/api/admin/chat${toSearch(filter)}`, { topics: ['chat'], keys: (data) => data.rows.map((row) => row.id) }));
  $effect(() => chat.watch());
  // Last good read stays on screen while a new filter loads.
  let last = $state<Chat | null>(null);
  $effect(() => {
    if (chat.data) last = chat.data;
  });
  // A refused filter shows its error alone; `last` still feeds the filter facets.
  const view = $derived(chat.error ? null : last);

  function apply(next: LogFilter) {
    onsearch?.(toSearch(next));
  }

  // The title bar's text search, debounced into the URL.
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

  let page = $state(1);
  $effect(() => {
    void search;
    page = 1;
  });
  const rows = $derived(view?.rows ?? []);
  const shown = $derived(paged(rows, page));
  const filtered = $derived(activeCount(filter) > 0);

  // Below 900 px the list and the turn take turns (as the Inbox).
  let narrow = $state(false);
  $effect(() => {
    const media = window.matchMedia(SINGLE_PANE_QUERY);
    const update = () => (narrow = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  // The default open turn stays put when a refresh adds turns above it.
  const firstOpen = pinnedFirst(() => chat);
  const chosenId = $derived(id || (narrow ? '' : firstOpen(shown.rows, page)));

  // The phone frame's open turn: no page line or window chrome; "‹ Chat" in the top bar.
  const chrome = getChrome();
  const compact = $derived(narrow && Boolean(id) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Chat', name: 'Back to the list (Chat)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  // Whether the open turn's entry came from a pick here (so leaving pops it) or a deep link.
  const pushedHere = () => (history.state as { chatDetail?: boolean } | null)?.chatDetail === true;
  function leaveDetail() {
    if (pushedHere()) history.back();
    else onselect?.('', false);
  }

  function pick(next: string, open: boolean) {
    onselect?.(next, open && narrow);
  }

  // Narrow screens swap list and turn: focus follows into the turn after a
  // pick, and back to the opened row on return; the list comes back backward.
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detailEl = $state<HTMLDivElement>();
  let returns = $state(0);
  let was = untrack(() => id);
  $effect(() => {
    const now = id;
    const before = was;
    was = now;
    if (!narrow || now === before) return;
    if (now && !before) void tick().then(() => detailEl?.focus({ preventScroll: true }));
    else if (!now && before) {
      untrack(() => returns++);
      void list?.focusOn(before);
    }
  });
</script>

<PageLine title={view ? 'Chat' : ''} class={compact ? 'pageline--echo' : ''}>
  <h1>{#if view}{#if filtered}<span class="pageline__num">{rows.length}</span> of <span class="pageline__num">{view.total}</span>{:else}<span class="pageline__num">{view.total}</span>{/if} interactions{:else}Chat{/if}</h1>
  {#snippet side()}
    {#if view && view.summary.length}<ModelStats summary={view.summary} />{/if}
  {/snippet}
</PageLine>

<section class="card chat window-fill" class:chat--compact={compact} data-fid="window" aria-labelledby="chat-window-title">
  <div class="card__head chat__bar" data-fid="window-bar">
    <h2 class="card__title" id="chat-window-title">Interactions</h2>
    <div class="chat__actions">
      <div class="chat__search" role="search" data-fid="window-search">
        <label class="vh" for="chat-q">Search interactions</label>
        <input id="chat-q" type="search" bind:value={query} placeholder="question, answer…" autocomplete="off" spellcheck="false" />
      </div>
      <div class="chat__filters" data-fid="window-filters">
        <LogFilters {filter} facets={last?.facets ?? null} members={store.members} week={store.week} chat onchange={apply} />
      </div>
    </div>
  </div>
  <div class="chat__body" class:chat__body--detail={narrow && Boolean(id)} class:chat__body--list={narrow && !id}>
    <div class="chat__list" data-fid="chat-list" hidden={narrow && Boolean(id)} {@attach enter(returns || null, 'backward')}>
      {#if chat.error}
        <LoadError thing="interactions" reason={chat.error} onretry={() => void chat.load()} level={3} />
      {:else if view}
        {#if rows.length === 0}
          <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
        {:else}
          <ChatList bind:this={list} rows={shown.rows} selected={chosenId} follow={!narrow} timeZone={tz} onpick={pick} fresh={chat.fresh} />
          <Pager bind:page pages={shown.pages} total={rows.length} noun="interaction" />
        {/if}
      {:else}
        <LoadingState text="Loading interactions…" />
      {/if}
    </div>
    <div class="chat__detail" data-fid="chat-detail" hidden={!chosenId} tabindex="-1" bind:this={detailEl}>
      {#if chosenId}
        {#if narrow && !compact}
          <button type="button" class="btn btn--ghost chat__back" onclick={leaveDetail}><span aria-hidden="true">←</span> Back to the list</button>
        {/if}
        <ChatTurn id={chosenId} timeZone={tz} {toaster} />
      {/if}
    </div>
  </div>
</section>
