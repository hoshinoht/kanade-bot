<!--
  Reminders (B_Reminders): every card the bot will post or already posted, in
  one window. Title-bar tabs Queued / Sent / Stale & other, the search and
  "Filters (n)"; one day-grouped table per tab, each keeping its own scroll
  position; the footer names the next card and how many rows are shown. A
  row opens its card preview beside the list (a sheet below 900 px). Relative
  times run on the server's clock (`generated_at`), never the browser's.
-->
<script lang="ts">
  import '@kanade/ui/styles/reminders.scss';
  import { tick } from 'svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import type { ReminderRow, Reminders } from '@kanade/api-types';
  import { LoadError, LoadingState, Presence, TWO_PANE_QUERY } from '@kanade/ui';
  import { artUrl } from '../shared/identity';
  import ReminderPane from './ReminderPane.svelte';
  import { Resource } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import ReminderFilters, { NO_FILTER, type ReminderFilter } from './ReminderFilters.svelte';
  import ReminderTable from './ReminderTable.svelte';
  import { dayOf, serverNow, span } from './when';

  let { store, run = '' }: { store?: AdminWeek; run?: string } = $props();

  const reminders = new Resource<Reminders>('/api/admin/reminders', { topics: ['schedule', 'delivery'] });
  $effect(() => reminders.watch());

  // Relative times: the server's now when the list arrived, advanced by
  // monotonic time (never the browser's wall clock), refreshed each half minute.
  const chrome = getChrome();
  const zone = $derived(chrome?.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone);
  let receivedAt = $state(performance.now());
  let monotonic = $state(performance.now());
  $effect(() => {
    if (reminders.data) receivedAt = monotonic = performance.now();
  });
  $effect(() => {
    const id = setInterval(() => (monotonic = performance.now()), 30_000);
    return () => clearInterval(id);
  });
  const now = $derived(reminders.data ? serverNow(reminders.data.generated_at, receivedAt, monotonic) : null);


  let query = $state('');
  const q = $derived(query.trim().toLowerCase());
  let filter = $state<ReminderFilter>({ ...NO_FILTER });
  const runName = (row: ReminderRow) => `${row.bosses.map((b) => b.token).join(' + ')} #${row.run_short_id}`;
  const all = $derived([...(reminders.data?.upcoming ?? []), ...(reminders.data?.sent ?? [])]);
  const distinct = (values: string[]) => [...new Set(values)];
  const kinds = $derived(distinct(all.map((r) => r.kind)));
  // The Run filter groups runs by the day of their last card, with the run's time and queued count (P_Select).
  const runs = $derived(
    distinct(all.map((r) => r.run_id)).map((id) => {
      const cards = all.filter((r) => r.run_id === id);
      const time = (store?.week?.runs.find((r) => r.id === id) ?? store?.thisWeek?.runs.find((r) => r.id === id))?.time;
      const queued = cards.filter((r) => r.state === 'queued' || r.state === 'due').length;
      const parts = [time, queued ? `${queued} queued` : 'none queued'].filter(Boolean);
      return { id, label: runName(cards[0]!), day: dayOf(cards[cards.length - 1]!.at), sub: parts.join(' · ') };
    }).sort((a, b) => days.indexOf(a.day) - days.indexOf(b.day)),
  );
  const people = $derived(distinct(all.flatMap((r) => r.party)).sort((a, b) => a.localeCompare(b)));
  const days = $derived(distinct(all.map((r) => dayOf(r.at))));
  const keep = (row: ReminderRow) =>
    (!run || row.run_id === run) &&
    (!q || [row.kind, row.run_short_id, ...row.party, ...row.bosses.flatMap((b) => [b.token, b.name])].some((t) => t.toLowerCase().includes(q))) &&
    (!filter.kind || row.kind === filter.kind) &&
    (!filter.run || row.run_id === filter.run) &&
    (!filter.member || row.party.includes(filter.member)) &&
    (!filter.day || dayOf(row.at) === filter.day);

  type Tab = 'queued' | 'sent' | 'stale';
  const TABS: { id: Tab; label: string; caption: string; empty: string }[] = [
    { id: 'queued', label: 'Queued', caption: 'Queued reminders', empty: 'Nothing pending' },
    { id: 'sent', label: 'Sent', caption: 'Sent reminders', empty: 'Nothing posted yet' },
    { id: 'stale', label: 'Stale & other', caption: 'Stale and other reminders', empty: 'Nothing stale' },
  ];
  const lists = $derived<Record<Tab, ReminderRow[]>>({
    queued: reminders.data?.upcoming ?? [],
    sent: (reminders.data?.sent ?? []).filter((r) => r.state === 'sent'),
    stale: (reminders.data?.sent ?? []).filter((r) => r.state !== 'sent'),
  });
  let tab = $state<Tab>('queued');
  const current = $derived(TABS.find((t) => t.id === tab)!);
  const total = $derived(lists[tab].filter((r) => !run || r.run_id === run).length);
  const shown = $derived(lists[tab].filter(keep));
  const narrowed = $derived(Boolean(q || Object.values(filter).some(Boolean)));
  const next = $derived(lists.queued.find((r) => r.state === 'queued' && (!run || r.run_id === run)) ?? null);
  const runLabel = $derived(run ? (all.find((r) => r.run_id === run)?.run_short_id ?? run) : '');

  // The card preview: a side pane from 900 px, a sheet below; focus goes back to the row.
  let openId = $state<string | null>(null);
  let wide = $state(false);
  $effect(() => {
    const query = window.matchMedia(TWO_PANE_QUERY);
    const update = () => (wide = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  const opened = $derived(openId ? (all.find((r) => r.id === openId) ?? null) : null);
  const pane = new Presence<ReminderRow>();
  $effect(() => pane.set(opened));
  let restore = '';
  function open(row: ReminderRow) {
    restore = row.id;
    openId = row.id;
  }
  function close() {
    openId = null;
    requestAnimationFrame(() =>
      document.querySelector<HTMLButtonElement>(`[data-reminder="${CSS.escape(restore)}"] .reminder__open`)?.focus({ preventScroll: true }),
    );
  }
  const bot = $derived(store?.identity?.name ?? 'Kanade');
  const avatar = $derived(store?.identity ? artUrl(store.identity.avatar, store.identity) : null);

  // Each tab keeps its own scroll position.
  let scroller = $state<HTMLDivElement>();
  const scrolls: Record<Tab, number> = { queued: 0, sent: 0, stale: 0 };
  async function choose(next: Tab) {
    if (next === tab) return;
    scrolls[tab] = scroller?.scrollTop ?? 0;
    tab = next;
    await tick();
    scroller?.scrollTo(0, scrolls[next]);
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const to = TABS[(target + TABS.length) % TABS.length]!;
    void choose(to.id);
    document.getElementById(`reminders-tab-${to.id}`)?.focus({ preventScroll: true });
  }
</script>

<PageLine title={reminders.data ? 'Reminders' : ''}>
  <h1>{#if reminders.data}<span class="pageline__num">{lists.queued.length}</span> queued · <span class="pageline__num">{lists.sent.length}</span> sent{:else}Reminders{/if}</h1>
  {#snippet side()}
    {#if run}
      <div class="page-head__side">
        <span class="chip chip--mono">run #{runLabel}</span>
        <a class="btn" href="/reminders">Show every run</a>
      </div>
    {/if}
  {/snippet}
</PageLine>

<section class="card reminders-window window-fill" data-fid="window" aria-labelledby="reminders-title">
  <div class="card__head tabs__strip reminders-window__head" data-fid="window-bar">
    <h2 class="vh" id="reminders-title">Reminders</h2>
    <div class="tabs__tabs" role="tablist" aria-label="Reminders" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        {@const n = lists[t.id].length}
        <button
          class="tabs__tab"
          role="tab"
          type="button"
          id="reminders-tab-{t.id}"
          aria-selected={tab === t.id}
          aria-controls="reminders-panel"
          tabindex={tab === t.id ? 0 : -1}
          onclick={() => void choose(t.id)}
          onkeydown={(event) => tabKey(event, index)}
          >{t.label}{#if reminders.data}<span class="tabs__count" class:reminders-window__count--warn={t.id === 'stale' && n > 0}>{n}</span>{/if}</button
        >
      {/each}
    </div>
    <div class="reminders-window__actions" data-fid="reminders-actions">
      <div class="reminders-window__search" data-fid="window-search" role="search">
        <label class="vh" for="reminders-q">Search reminders</label>
        <input id="reminders-q" type="search" bind:value={query} placeholder="boss, member, run id…" autocomplete="off" spellcheck="false" />
      </div>
      <ReminderFilters bind:filter {kinds} {runs} {people} {days} />
    </div>
  </div>
  <div class="reminders-window__body">
  <div
    class="reminders-window__list"
    data-fid="reminders-list"
    id="reminders-panel"
    role="tabpanel"
    aria-labelledby="reminders-tab-{tab}"
    tabindex="0"
    bind:this={scroller}
  >
    {#if reminders.error}
      <LoadError thing="reminders" reason={reminders.error} onretry={() => void reminders.load()} />
    {:else if reminders.data}
      {#if shown.length}
        <ReminderTable rows={shown} {tab} caption={current.caption} {now} {zone} active={openId} onopen={open} />
      {:else}
        <p class="empty">{current.empty}{narrowed ? ' that matches' : ''}.</p>
      {/if}
    {:else}
      <LoadingState text="Loading reminders…" />
    {/if}
  </div>
  {#if pane.shown}
    <ReminderPane {wide} row={opened ?? pane.shown} {bot} {avatar} onclose={close} leaving={pane.leaving} onleft={(event) => pane.done(event)} />
  {/if}
  </div>
  <footer class="reminders-window__foot" data-fid="reminders-foot">
    {#if next}
      <span class="cap">Next</span><b class="mono reminders-window__next">in {now === null ? '' : span(next.fire_at, now, zone)}</b><span>{next.kind} · {next.bosses.map((b) => b.token).join(' + ')}</span>
    {:else if reminders.data}
      <span>Nothing queued</span>
    {/if}
    {#if reminders.data}<span class="reminders-window__shown">{shown.length} of {total} shown</span>{/if}
  </footer>
</section>
