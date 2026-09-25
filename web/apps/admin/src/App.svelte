<script lang="ts">
  import { tick } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import {
    applyColorway,
    applyMode,
    COLORWAYS,
    Freshness,
    Icon,
    Masthead,
    registerServiceWorker,
    runFullTitle,
    ToastRegion,
    Toaster,
    whenLabel,
    type Command,
  } from '@kanade/ui';
  import Lazy from './pages/Lazy.svelte';
  import NotFoundPage from './pages/NotFoundPage.svelte';
  import WeekPage, { type WeekTab } from './pages/WeekPage.svelte';
  import type { Run } from '@kanade/api-types';
  import type { Slot } from './planner/keyboardMove';
  import { match, Router } from './router.svelte';
  import { DETAILS, ROUTES, SECTIONS } from './routes';
  import type RunSheetType from './RunSheet.svelte';
  import type { default as PaletteType } from '@kanade/ui/palette';
  import Nav from './shell/Nav.svelte';
  import { AdminWeek, type MoveOutcome } from './store.svelte';
  import { reread } from './week/reread';

  // Route-level code splitting: only the Week page (and the run sheet it
  // opens) is in the initial bundle; every other page loads on first visit.
  const PAGES = {
    login: () => import('./pages/LoginPage.svelte'),
    fixed: () => import('./fixed/FixedPage.svelte'),
    bosses: () => import('./bosses/BossesPage.svelte'),
    'boss-knowledge': () => import('./bosses/KnowledgePage.svelte'),
    inbox: () => import('./inbox/InboxPage.svelte'),
    extractions: () => import('./extractions/ExtractionsPage.svelte'),
    extraction: () => import('./extractions/ExtractionPage.svelte'),
    chat: () => import('./chat/ChatPage.svelte'),
    'chat-interaction': () => import('./chat/ChatTurnPage.svelte'),
    limits: () => import('./limits/LimitsPage.svelte'),
    members: () => import('./members/MembersPage.svelte'),
    reminders: () => import('./reminders/RemindersPage.svelte'),
    config: () => import('./config/ConfigPage.svelte'),
    history: () => import('./history/HistoryPage.svelte'),
  } as const;

  const router = new Router();
  const store = new AdminWeek();
  const toaster = new Toaster();
  let weekTab = $state<WeekTab>('planner');
  let paletteOpen = $state(false);
  // The palette loads on its first Ctrl/Cmd-K.
  let Palette = $state<typeof PaletteType | null>(null);
  async function togglePalette(force?: boolean) {
    Palette ??= (await import('@kanade/ui/palette')).default;
    paletteOpen = force ?? !paletteOpen;
  }
  let sheetOpen = $state(false);
  let sheetRunId = $state<string | null>(null);

  const route = $derived(match(router.path, ROUTES));
  const detail = $derived(DETAILS.find((d) => d.key === route?.key));
  const section = $derived(SECTIONS.find((s) => s.key === (detail?.section ?? route?.key)));
  const which = $derived(router.query.get('week') === 'next' ? 'next' : 'this');
  const sheetRun = $derived(sheetRunId ? (store.run(sheetRunId) ?? null) : null);
  const title = $derived(route?.key === 'login' ? 'Sign in' : (detail?.title ?? section?.title ?? 'Not found'));

  $effect(() => store.start());

  // v4's Audit page became History.
  $effect(() => {
    if (router.path === '/audit') router.go('/history', { replace: true });
  });

  function pageProps(key: string, params: Record<string, string>): Record<string, unknown> {
    switch (key) {
      case 'login':
        return { identity: store.identity, onsignin: () => router.go('/') };
      case 'fixed':
      case 'history':
        return { store, toaster };
      case 'inbox':
        return {
          store,
          toaster,
          tab: router.query.get('tab') ?? '',
          item: router.query.get('item') ?? '',
          onselect: (tab: string, item: string, open: boolean) =>
            router.go(`/inbox?tab=${encodeURIComponent(tab)}${item ? `&item=${encodeURIComponent(item)}` : ''}`, {
              replace: !open,
              state: open ? { inboxDetail: true } : null,
            }),
        };
      case 'limits':
        return { toaster };
      case 'chat':
      case 'extractions':
        return {
          store,
          search: router.search,
          onsearch: (search: string) => router.go(`/${key}${search}`, { replace: true }),
        };
      case 'config':
        return {
          toaster,
          section: router.query.get('section') ?? '',
          onsection: (key: string) => router.go(`/config?section=${key}`, { replace: true }),
        };
      case 'boss-knowledge':
        return { key: params.boss ?? '', difficulty: router.query.get('difficulty') ?? '' };
      case 'extraction':
      case 'chat-interaction':
        return { id: params.id ?? '' };
      case 'reminders':
        return { run: router.query.get('run') ?? '' };
      default:
        return {};
    }
  }
  const loader = $derived(route && route.key in PAGES ? PAGES[route.key as keyof typeof PAGES] : null);
  $effect(() => store.setWeek(which));

  $effect(() => {
    document.title = `${title} — ${store.identity?.name ?? 'Kanade'}`;
  });

  // A route change (not the first load) moves focus to the page, as a page load would.
  let lastPath = router.path;
  $effect(() => {
    const path = router.path;
    if (path === lastPath) return;
    lastPath = path;
    void tick().then(() => document.getElementById('main')?.focus());
  });

  const reloadToast = (message: string) =>
    toaster.show({ message, tone: 'error', timeoutMs: null, action: { label: 'Reload', run: () => location.reload() } });

  $effect(() => {
    void registerServiceWorker({
      url: '/sw.js',
      onUpdateReady: (apply) =>
        toaster.show({ message: 'A new version of Kanade Admin is ready.', timeoutMs: null, action: { label: 'Reload', run: apply } }),
      // Another tab accepted the update; this one keeps its state and is asked, not reloaded.
      onControllerChange: () => reloadToast('Kanade Admin was updated in another tab. Reload to use the new version.'),
    });
  });

  // A lazy chunk or its CSS failed to preload (typically a deploy removed it).
  // The import itself still rejects, so the Answers tab's {:catch} renders too.
  $effect(() => {
    const onPreloadError = () => reloadToast('Part of Kanade Admin failed to load, probably after an update.');
    window.addEventListener('vite:preloadError', onPreloadError);
    return () => window.removeEventListener('vite:preloadError', onPreloadError);
  });

  // The run sheet (and its history panel) loads on first open, not with the page.
  let RunSheet = $state<typeof RunSheetType | null>(null);
  async function openSheet(runId: string) {
    if (!store.run(runId)) return;
    RunSheet ??= (await import('./RunSheet.svelte')).default;
    sheetRunId = runId;
    sheetOpen = true;
  }

  function report(outcome: MoveOutcome, undo?: () => void) {
    toaster.show({
      message: outcome.message,
      tone: outcome.ok ? 'ok' : 'error',
      // Ten seconds, paused on hover/focus; Ctrl/Cmd+Z and the palette also undo moves.
      timeoutMs: outcome.ok ? 10_000 : null,
      action: outcome.ok && undo ? { label: 'Undo', run: undo } : undefined,
    });
  }

  async function move(runId: string, to: Slot): Promise<MoveOutcome> {
    const outcome = await store.move(runId, to);
    report(outcome, () => void undo());
    return outcome;
  }

  async function undo() {
    const runId = store.lastMove?.runId;
    const prior = document.activeElement;
    const outcome = await store.undo();
    if (!outcome) return;
    report(outcome);
    // Undo from a toast removes the button that had focus; land on the moved
    // card's handle instead of the document body, as does the page-head button,
    // which disables itself. Ctrl/Cmd-Z elsewhere keeps focus where it was.
    await tick();
    const kept = prior instanceof HTMLElement && prior.isConnected && prior !== document.body && !prior.matches(':disabled');
    if (kept) return;
    const handle = runId ? document.querySelector<HTMLElement>(`[data-handle="${CSS.escape(runId)}"]`) : null;
    (handle ?? document.getElementById('main'))?.focus();
  }

  // One re-read per channel at a time; the button shows it with aria-disabled.
  // The board and the run sheet share this guard, so one channel never runs twice.
  const rereading = new SvelteSet<string>();
  async function rereadChannel(run: Run): Promise<MoveOutcome> {
    if (rereading.has(run.channel_id)) return { ok: false, message: `Already re-reading ${run.channel}.` };
    rereading.add(run.channel_id);
    try {
      return await reread(run.channel_id, run.channel);
    } finally {
      rereading.delete(run.channel_id);
    }
  }
  async function rereadFromBoard(run: Run) {
    if (rereading.has(run.channel_id)) return;
    const pending = toaster.show({ message: `Re-reading ${run.channel}…`, timeoutMs: null });
    const outcome = await rereadChannel(run);
    toaster.dismiss(pending);
    toaster.show({ message: outcome.message, tone: outcome.ok ? 'ok' : 'error', timeoutMs: outcome.ok ? 8000 : null });
  }

  const commands = $derived<Command[]>([
    ...SECTIONS.map((s) => ({ id: `go-${s.key}`, label: `Go to ${s.label}`, group: 'Page', keywords: s.group, run: () => router.go(s.href) })),
    ...(['planner', 'runs', 'answers'] as const).map((id) => ({
      id: `tab-${id}`,
      label: `Show ${id === 'planner' ? 'Planner' : id === 'runs' ? 'Runs' : 'Answers'}`,
      group: 'Week',
      run: () => {
        router.go(which === 'next' ? '/?week=next' : '/');
        weekTab = id;
      },
    })),
    { id: 'week-next', label: 'Show next week', group: 'Week', run: () => router.go('/?week=next') },
    { id: 'week-this', label: 'Show this week', group: 'Week', run: () => router.go('/') },
    ...(store.lastMove ? [{ id: 'undo', label: 'Undo last move', group: 'Edit', keywords: 'revert', run: () => void undo() }] : []),
    { id: 'refresh', label: 'Refresh now', group: 'Data', keywords: 'reload poll', run: () => void store.refresh() },
    ...(store.week?.runs ?? []).map((run) => ({
      id: `run-${run.id}`,
      label: `Open ${runFullTitle(run)}`,
      group: 'Run',
      keywords: `${run.bosses.map((b) => b.token).join(' ')} ${whenLabel(store.week!, run.day, run.time)} ${run.party}`,
      run: () => openSheet(run.id),
    })),
    ...COLORWAYS.map((way) => ({
      id: `colorway-${way.key}`,
      label: `Colourway: ${way.name}`,
      group: 'Theme',
      keywords: 'theme colour color',
      run: () => applyColorway(way.key),
    })),
    ...(['system', 'light', 'dark'] as const).map((mode) => ({
      id: `mode-${mode}`,
      label: `Mode: ${mode[0]!.toUpperCase()}${mode.slice(1)}`,
      group: 'Theme',
      keywords: 'theme dark light night',
      run: () => applyMode(mode),
    })),
  ]);

  function typing(target: EventTarget | null): boolean {
    return target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
  }

  function onKeydown(event: KeyboardEvent) {
    const mod = event.metaKey || event.ctrlKey;
    if (mod && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      void togglePalette();
    } else if (mod && event.key.toLowerCase() === 'z' && !event.shiftKey && !typing(event.target) && !paletteOpen && !sheetOpen) {
      if (store.lastMove) {
        event.preventDefault();
        void undo();
      }
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

{#if route?.key === 'login'}
  <main id="main" tabindex="-1">
    <Lazy loader={PAGES.login} props={pageProps('login', {})} />
  </main>
{:else}
  <div class="frame">
    <a class="skip" href="#main">Skip to the page</a>
    <Masthead name={store.identity?.name ?? 'Kanade'} avatar={store.identity?.avatar ?? null} href="/">
      {#snippet meta()}
        {#if store.week}<span class="masthead__tz" title="Guild timezone — every time here is in it">{store.week.timezone}</span>{/if}
        <a class="brand__by" href="https://github.com/hoshinoht/kanade-bot" rel="noopener noreferrer" target="_blank">powered by kanade</a>
        <Freshness state={store.fresh} updated={store.updated} />
        {#if store.session}<span class="masthead__who masthead__desk-only">{store.session.display}</span>{/if}
        <a class="masthead__desk-only" href="/login">sign out</a>
        <button type="button" class="btn btn--ghost masthead__desk-only" onclick={() => void togglePalette(true)} aria-keyshortcuts="Control+K Meta+K">
          <Icon name="search" /> Commands <kbd class="kbd">Ctrl K</kbd>
        </button>
      {/snippet}
      {#snippet nav()}
        <Nav active={section?.key ?? ''} inbox={store.summary?.inbox ?? 0} />
      {/snippet}
    </Masthead>
    <main class="shell" id="main" tabindex="-1">
      {#if route?.key === 'week'}
        <WeekPage
          {store}
          {which}
          bind:tab={weekTab}
          onmove={(runId, to) => void move(runId, to)}
          onopen={openSheet}
          onundo={() => void undo()}
          onreread={(run) => void rereadFromBoard(run)}
          busyChannels={rereading}
        />
      {:else if loader && route}
        {#key `${route.key} ${JSON.stringify(route.params)}`}<Lazy {loader} props={pageProps(route.key, route.params)} />{/key}
      {:else}
        <NotFoundPage path={router.path} />
      {/if}

      <p class="footnote">
        <span>All times {store.week?.timezone ?? 'Asia/Kuala_Lumpur'}.</span>
        <span>Boss week starts {store.week?.reset ?? 'Thu 00:00'}.</span>
        <span>Moves apply to the week on screen only.</span>
      </p>
    </main>
    {#if store.week && RunSheet}
      <RunSheet
        bind:open={sheetOpen}
        run={sheetRun}
        week={store.week}
        members={store.members}
        onmove={(runId, to) => move(runId, to)}
        onstatus={(runId, status) => store.setStatus(runId, status)}
        onrsvp={(runId, memberId, answer) => store.rsvp(runId, memberId, answer)}
        onroster={(runId, change) => store.roster(runId, change)}
        onreset={(runId) => store.resetToFixed(runId)}
        onping={(runId) => store.ping(runId)}
        onreread={rereadChannel}
      />
    {/if}
    {#if Palette}<Palette bind:open={paletteOpen} {commands} />{/if}
  </div>
{/if}
<ToastRegion {toaster} />
