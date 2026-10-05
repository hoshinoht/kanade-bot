<!--
  v5 Limits (B_LimitsLive and its tab boards): the Kanata gateway's backend
  groups, the calls waiting for a permit, what the gateway refused, and the
  chat allowances, in one window with title-bar tabs. Backends stacks one
  row card per group in the configured order (phones: open, then half-open
  breakers first); only the selected tab's panel scrolls. Polled every 5 s;
  the footer prints the server's clock (`generated_at`), never the browser's.
  A failed first read shows the shared failed state; a failed refresh keeps
  the snapshot under a Retrying chip (phones: under the tabs).
-->
<script lang="ts">
  import { tick } from 'svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import '@kanade/ui/styles/limits.scss';
  import type { Limits } from '@kanade/api-types';
  import { ApiRequestError, createClient, createPoller } from '@kanade/client';
  import { clockTime, Freshness, Icon, LoadError, LoadingState, Toaster } from '@kanade/ui';
  import { errorText, send } from '../resource.svelte';
  import GroupCard from './GroupCard.svelte';
  import QueueTab from './QueueTab.svelte';
  import AdmissionTab from './AdmissionTab.svelte';
  import AllowancesTab from './AllowancesTab.svelte';
  import { wavingGroups } from './permits';
  import { atCapacity, GUILD_ZONE, phoneOrder, plural, serverTime } from './view';

  let { toaster }: { toaster: Toaster } = $props();

  const chrome = getChrome();
  const phone = $derived(chrome?.phone ?? false);
  const zone = $derived(chrome?.timezone || GUILD_ZONE);

  let limits = $state<Limits | null>(null);
  // A failed refresh keeps the last snapshot and says so where the Live chip was.
  let fresh = $state<'live' | 'stale' | 'offline'>('live');
  // After `maxFailures` the poller gives up until Try again; the chip stops saying "Retrying".
  let stopped = $state(false);
  // A failed first read (not a 404): the window shows the shared failed state.
  let failure = $state('');
  // A 404 means this server has not built the route: stop polling and say so in the window.
  let unbuilt = $state('');
  const client = createClient();
  const poller = createPoller<Limits>({
    task: (signal) => client.get<Limits>('/api/admin/limits', { signal }),
    intervalMs: 5000,
    maxFailures: 6,
    onState: (state) => (stopped = state === 'stopped'),
    onData: (next) => {
      limits = next;
      fresh = 'live';
      failure = '';
    },
    onError: (e) => {
      if (e instanceof ApiRequestError && e.status === 404) {
        poller.stop();
        unbuilt = errorText(e);
        return;
      }
      fresh = typeof navigator !== 'undefined' && navigator.onLine === false ? 'offline' : 'stale';
      if (!limits) failure = e instanceof ApiRequestError ? errorText(e) : e instanceof Error ? e.message : String(e);
    },
  });
  const retry = () => void poller.refresh();
  const lastClock = $derived(limits?.generated_at ? clockTime(limits.generated_at, zone, false) : '');
  $effect(() => {
    poller.start();
    return () => poller.stop();
  });

  type Tab = 'backends' | 'queue' | 'admission' | 'allowances';
  const groups = $derived(limits?.groups ?? []);
  const queued = $derived(groups.reduce((n, g) => n + g.queue.length, 0));
  const refused = $derived(limits?.admission.refusals.reduce((n, r) => n + r.count, 0) ?? 0);
  const TABS = $derived<{ id: Tab; label: string; count: number }[]>([
    { id: 'backends', label: 'Backends', count: groups.length },
    { id: 'queue', label: 'Queue', count: queued },
    { id: 'admission', label: 'Admission', count: refused },
    { id: 'allowances', label: 'Allowances', count: limits?.allowances.length ?? 0 },
  ]);
  let tab = $state<Tab>('backends');

  // Permit bars wave only while requests are in flight, at most two at once.
  const waving = $derived(wavingGroups(groups));
  const shown = $derived(phone ? phoneOrder(groups) : groups);
  const busiest = $derived(atCapacity(groups));
  const open = $derived(groups.filter((g) => g.breaker.state === 'open').length);
  const inUse = $derived(groups.reduce((n, g) => n + g.permits.in_use, 0));
  const total = $derived(groups.reduce((n, g) => n + g.permits.total, 0));
  const oldest = $derived(
    groups
      .flatMap((g) => g.queue.map((q) => ({ ...q, group: g.name })))
      .reduce<{ waiting_s: number; kind: string; group: string } | null>((a, q) => (!a || q.waiting_s > a.waiting_s ? q : a), null),
  );
  const updated = $derived(limits?.generated_at ? serverTime(limits.generated_at, zone) : '');
  const ALLOWANCE_NOTE = 'Members with chatbot access. Staff are exempt from every budget. The member-facing view comes later.';

  // Each tab keeps its own scroll position.
  let scroller = $state<HTMLDivElement>();
  const scrolls: Record<Tab, number> = { backends: 0, queue: 0, admission: 0, allowances: 0 };
  async function choose(next: Tab) {
    if (next === tab) return;
    scrolls[tab] = scroller?.scrollTop ?? 0;
    tab = next;
    await tick();
    scroller?.scrollTo(0, scrolls[next]);
    reveal(next);
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const to = TABS[(target + TABS.length) % TABS.length]!;
    void choose(to.id);
    document.getElementById(`limits-tab-${to.id}`)?.focus({ preventScroll: true });
  }

  // Phones scroll the tab strip sideways; fades mark the side that has more.
  let strip = $state<HTMLDivElement>();
  let more = $state({ start: false, end: false });
  function measure() {
    if (!strip) return;
    const { scrollLeft, scrollWidth, clientWidth } = strip;
    more = { start: scrollLeft > 1, end: scrollLeft + clientWidth < scrollWidth - 1 };
  }
  // Bring the chosen tab fully into the strip without scrolling anything else.
  function reveal(id: Tab) {
    const el = document.getElementById(`limits-tab-${id}`);
    if (!strip || !el) return;
    const pad = 24;
    if (el.offsetLeft - pad < strip.scrollLeft) strip.scrollLeft = Math.max(0, el.offsetLeft - pad);
    else if (el.offsetLeft + el.offsetWidth + pad > strip.scrollLeft + strip.clientWidth)
      strip.scrollLeft = el.offsetLeft + el.offsetWidth + pad - strip.clientWidth;
    measure();
  }
  $effect(() => {
    if (!strip) return;
    const observer = new ResizeObserver(measure);
    observer.observe(strip);
    return () => observer.disconnect();
  });

  /** Resolves once the refreshed snapshot (without that Reset button) has rendered; true when the reset held. */
  async function reset(id: string, name: string): Promise<boolean> {
    const result = await send((c) => c.delete<{ message: string }>(`/api/admin/limits/windows/${encodeURIComponent(id)}`));
    toaster.show({ message: result.ok ? result.value.message : `Couldn't reset ${name}: ${result.message}`, tone: result.ok ? 'ok' : 'error' });
    if (!result.ok) return false;
    await poller.refresh();
    await tick();
    return true;
  }
</script>

<PageLine
  title={limits ? 'Limits' : ''}
  class="{limits && fresh !== 'live' ? 'limits-pageline--stale' : ''} {limits && phone ? 'pageline--echo' : ''}"
>
  <h1>{limits ? (busiest ? `${busiest.name} is at capacity` : groups.length ? 'Every backend has room' : 'Limits') : 'Limits'}</h1>
  {#if limits && (queued || open)}
    <p class="pageline__context" data-fid="limits-headline">
      {#if queued}<span class="pageline__num">{queued}</span> waiting{/if}{#if queued && open}&nbsp;·&nbsp;{/if}{#if open}<span
          class="pageline__num">{open}</span
        > {plural(open, 'breaker')} open{/if}
    </p>
  {/if}
  {#if unbuilt}<p class="pageline__context">capacity</p>{/if}
  {#snippet side()}
    {#if limits && fresh !== 'live' && !phone}{@render freshness()}{/if}
  {/snippet}
</PageLine>

<!-- A failed refresh keeps the last snapshot; after the poller gives up the chip says so and offers Try again. -->
{#snippet freshness()}
  <span class="mchip mchip--status limits-fresh" role="status">
    {#if stopped}
      <span class="fresh fresh--error" data-fresh="stopped"
        ><Icon name="alert-circle" /> Refreshing stopped{#if lastClock}&nbsp;· last updated {lastClock}{/if}</span
      >
    {:else}
      <Freshness state={fresh} updated={lastClock} />
    {/if}
  </span>
  {#if stopped}<button type="button" class="btn limits-fresh__retry" onclick={retry}>Try again</button>{/if}
{/snippet}

{#if limits}
  <section class="card limits-window window-fill" data-fid="window" aria-labelledby="limits-title">
    <div class="card__head tabs__strip limits-window__head" data-fid="window-bar">
      <h2 class="vh" id="limits-title">Limits</h2>
      <div
        class="tabs__tabs limits-window__tabs"
        class:limits-window__tabs--start={more.start}
        class:limits-window__tabs--end={more.end}
        role="tablist"
        aria-label="Limits"
        data-fid="window-tabs"
        bind:this={strip}
        onscroll={measure}
      >
        {#each TABS as t, index (t.id)}
          <button
            class="tabs__tab"
            role="tab"
            type="button"
            id="limits-tab-{t.id}"
            aria-selected={tab === t.id}
            aria-controls="limits-panel"
            tabindex={tab === t.id ? 0 : -1}
            onclick={() => void choose(t.id)}
            onkeydown={(event) => tabKey(event, index)}>{t.label}<span class="tabs__count">{t.count}</span></button
          >
        {/each}
      </div>
      {#if !phone}
        <a class="btn limits-window__config" data-fid="limits-config" href="/config?section=models"><Icon name="sliders" />Capacity in Config</a>
      {/if}
    </div>
    {#if phone}
      <div class="limits-window__summary" data-fid="limits-phone-head">
        <p>
          {#if tab === 'admission'}
            Refused by the Kanata gateway in the <b>{limits.admission.window}</b>.
          {:else if tab === 'allowances'}
            {ALLOWANCE_NOTE}
          {:else if busiest}
            <b>{busiest.name}</b> is at capacity{#if queued}&nbsp;· <b class="mono">{queued}</b> waiting{/if}{#if open}&nbsp;· <b class="mono">{open}</b>
              {plural(open, 'breaker')} open{/if}
          {:else}
            {groups.length ? 'Every backend has room' : 'No model groups are running'}{#if queued}&nbsp;· <b class="mono">{queued}</b> waiting{/if}
          {/if}
        </p>
        <a class="btn limits-window__config" data-fid="limits-config" href="/config?section=models" aria-label="Capacity in Config"><Icon name="sliders" /></a>
      </div>
      <!-- The phone's page line is hidden and its top-bar chip follows the week, so Limits' own freshness shows here. -->
      {#if fresh !== 'live'}<div class="limits-window__fresh" data-fid="limits-phone-fresh">{@render freshness()}</div>{/if}
    {/if}
    <div
      class="limits-window__panel"
      class:limits-window__panel--cards={tab === 'backends'}
      class:limits-window__panel--phone={phone}
      data-fid="limits-panel"
      id="limits-panel"
      role="tabpanel"
      aria-labelledby="limits-tab-{tab}"
      tabindex="0"
      bind:this={scroller}
    >
      {#if tab === 'backends'}
        {#each shown as g (g.name)}
          <GroupCard group={g} wavy={waving.has(g.name)} {zone} {phone} />
        {:else}
          <p class="empty">No model groups are running.</p>
        {/each}
      {:else if tab === 'queue'}
        <QueueTab {groups} {phone} />
      {:else if tab === 'admission'}
        <AdmissionTab refusals={limits.admission.refusals} {zone} {phone} />
      {:else}
        <AllowancesTab rows={limits.allowances} {phone} onreset={reset} />
      {/if}
    </div>
    <footer class="limits-window__foot" class:limits-window__foot--phone={phone} data-fid="limits-foot">
      <span class="limits-window__lead">
        {#if tab === 'backends'}
          {#if !phone}<b class="mono">{groups.length}</b> backend {plural(groups.length, 'group')} ·&nbsp;{/if}<b class="mono">{inUse}</b> of
          <span class="mono">{total}</span> permits in use
        {:else if tab === 'queue'}
          {#if oldest}<span class="cap">Oldest</span> <b class="mono limits-window__big">{oldest.waiting_s} s</b>{phone ? '' : ` ${oldest.kind} · ${oldest.group}`}{:else}Nothing is waiting{/if}
        {:else if tab === 'admission'}
          {#if phone}<b class="mono">{refused}</b> {plural(refused, 'refusal')}{:else}Refused by the Kanata gateway in the <b>{limits.admission.window}</b>.{/if}
        {:else if phone}
          <b class="mono">{limits.allowances.length}</b> {plural(limits.allowances.length, 'member')}
        {:else}
          {ALLOWANCE_NOTE}
        {/if}
      </span>
      {#if updated}<span class="limits-window__updated">Updated <span class="mono">{updated}</span> · every 5 s</span>{/if}
    </footer>
  </section>
{:else if unbuilt}
  <!-- B_Limits: the route is not mounted. One window, its body a centred note. -->
  <section class="card limits-window window-fill" data-fid="window" aria-labelledby="limits-title">
    <div class="card__head limits-window__head" data-fid="window-bar"><h2 class="card__title" id="limits-title">Limits</h2></div>
    <div class="limits-window__body" data-fid="limits-body">
      <div class="limits-window__centre">
        <div class="limits-unavailable" data-fid="limits-unavailable" role="status">
          <span class="limits-unavailable__mark" data-fid="limits-unavailable-mark" aria-hidden="true"><Icon name="gauge" /></span>
          <h3 class="limits-unavailable__title" data-fid="limits-unavailable-title">{unbuilt.replace(/\.$/, '')}</h3>
          <p class="limits-unavailable__text" data-fid="limits-unavailable-text">
            Live capacity, queues and gateway refusals will show here once the server supports them. Model backends and their capacity are still set
            in Config.
          </p>
          <a class="btn btn--primary limits-unavailable__link" data-fid="limits-unavailable-link" href="/config?section=models">Open Config → Models</a>
        </div>
      </div>
    </div>
  </section>
{:else if failure}
  <!-- A failed first read (5xx or network): the shared failed state; Try again restarts polling. -->
  <section class="card limits-window window-fill" data-fid="window" aria-labelledby="limits-title">
    <div class="card__head limits-window__head" data-fid="window-bar"><h2 class="card__title" id="limits-title">Limits</h2></div>
    <div class="limits-window__body" data-fid="limits-body"><LoadError thing="the limits" reason={failure} onretry={retry} level={3} /></div>
  </section>
{:else}
  <section class="card window-fill"><div class="card__head"><h2 class="card__title">Limits</h2></div><LoadingState text="Loading the limits…" /></section>
{/if}
