<script lang="ts">
  import { tick } from 'svelte';
  import { MediaQuery, SvelteSet } from 'svelte/reactivity';
  import {
    applyColorway,
    applyMode,
    COLORWAYS,
    experiments,
    registerServiceWorker,
    runFullTitle,
    setExperiments,
    setOvershoot,
    ToastRegion,
    Toaster,
    whenLabel,
    type Command,
  } from '@kanade/ui';
  import Lazy from './pages/Lazy.svelte';
  import NotFoundPage from './pages/NotFoundPage.svelte';
  import WeekPage, { type WeekTab } from './pages/WeekPage.svelte';
  import type { Run, Session } from '@kanade/api-types';
  import { onUnauthenticated } from '@kanade/client';
  import { loginHref, safeNext } from './auth';
  import type { Slot } from './planner/keyboardMove';
  import { match, Router } from './router.svelte';
  import { DETAILS, ROUTES, SECTIONS } from './routes';
  import type RunSheetType from './RunSheet.svelte';
  import type { default as PaletteType } from '@kanade/ui/palette';
  import AccountMenu from './shell/AccountMenu.svelte';
  import EdgeSwipe from './shell/EdgeSwipe.svelte';
  import { PHONE_QUERY, setChrome, type BackStep } from './shell/chrome';
  import NavDrawer from './shell/NavDrawer.svelte';
  import Rail from './shell/Rail.svelte';
  import TopBar from './shell/TopBar.svelte';
  import { directory } from './names/directory.svelte';
  import { artUrl } from './shared/identity';
  import { AdminWeek, type MoveOutcome } from './store.svelte';
  import { reread } from './week/reread';

  // Route-level code splitting: only the Week page (and the run sheet it
  // opens) is in the initial bundle; every other page loads on first visit.
  const loadBosses = () => import('./bosses/BossWorkspace.svelte');
  const PAGES = {
    login: () => import('./pages/LoginPage.svelte'),
    fixed: () => import('./fixed/FixedPage.svelte'),
    bosses: loadBosses,
    'boss-knowledge': loadBosses,
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

  // The phone frame (top bar + drawer) or the rail; exactly one is rendered.
  const phoneQuery = window.matchMedia(PHONE_QUERY);
  let phone = $state(phoneQuery.matches);
  let drawerOpen = $state(false);
  // Gate G4: from 840 px the run sheet is a side pane in the Week window; below, a full-screen sheet.
  const paneQuery = new MediaQuery('(min-width: 840px)', true);
  const sheetWide = $derived(paneQuery.current && !phone);
  let menuButton = $state<HTMLButtonElement>();
  $effect(() => {
    const update = () => {
      phone = phoneQuery.matches;
      if (!phone) drawerOpen = false;
    };
    phoneQuery.addEventListener('change', update);
    return () => phoneQuery.removeEventListener('change', update);
  });

  const route = $derived(match(router.path, ROUTES));
  const detail = $derived(DETAILS.find((d) => d.key === route?.key));
  const section = $derived(SECTIONS.find((s) => s.key === (detail?.section ?? route?.key)));
  const which = $derived(router.query.get('week') === 'next' ? 'next' : 'this');
  const sheetRun = $derived(sheetRunId ? (store.run(sheetRunId) ?? null) : null);
  const title = $derived(route?.key === 'login' ? 'Sign in' : (detail?.title ?? section?.title ?? 'Not found'));

  // A page's back step for the phone's top bar (an open Inbox item).
  let pageBack = $state<BackStep | null>(null);

  // What the page line needs from the shell (shell/PageLine.svelte).
  setChrome({
    get phone() {
      return phone;
    },
    get fresh() {
      return store.fresh;
    },
    get updated() {
      return store.updated;
    },
    get timezone() {
      return store.week?.timezone ?? '';
    },
    palette: () => void togglePalette(true),
    back: (step) => (pageBack = step),
  });

  // The Discord callback reports failures at `/?login_error=<code>`: show them on the sign-in page.
  $effect(() => {
    const code = router.query.get('login_error');
    if (code && route?.key !== 'login') router.go(`/login?login_error=${encodeURIComponent(code)}`, { replace: true });
  });

  // Signed out (or the session ended): sign in, then come back here.
  $effect(() => {
    onUnauthenticated((path) => {
      // A refused sign-in attempt is the form's to explain.
      if (path.startsWith('/api/admin/auth/') || router.path === '/login' || router.query.has('login_error')) return;
      store.session = null;
      router.go(loginHref(router.path + router.search), { replace: true });
    });
    return () => onUnauthenticated(null);
  });

  // No polling behind the sign-in page; signing in starts it (and re-reads the session).
  const signingIn = $derived(route?.key === 'login');
  $effect(() => (signingIn ? undefined : store.start()));

  // Session carries no user id; a Discord sign-in's display name is the member's, when it names exactly one.
  const accountId = $derived.by(() => {
    if (store.session?.method !== 'discord') return null;
    const matches = [...directory.members].filter(([, name]) => name === store.session?.display);
    return matches.length === 1 ? matches[0]![0] : null;
  });

  async function copyAccountId(id: string) {
    try {
      await navigator.clipboard.writeText(id);
      toaster.show({ message: 'Copied your user ID.', tone: 'ok' });
    } catch {
      toaster.show({ message: "Couldn't copy your user ID.", tone: 'error' });
    }
  }

  async function signOut(event: MouseEvent) {
    event.preventDefault();
    await store.signOut();
    router.go('/login');
  }

  // v4's Audit page became History.
  $effect(() => {
    if (router.path === '/audit') router.go('/history', { replace: true });
  });

  function pageProps(key: string, params: Record<string, string>): Record<string, unknown> {
    switch (key) {
      case 'login': {
        const next = safeNext(router.query.get('next'));
        return {
          next,
          loginError: router.query.get('login_error') ?? '',
          onsignedin: (session: Session) => {
            store.session = session;
            router.go(next, { replace: true });
          },
        };
      }
      case 'fixed':
        // `?open=<id>` (Bosses' weekly timings) opens that timing's editor once.
        return { store, toaster, openId: router.query.get('open') ?? '', onopened: () => router.go('/fixed', { replace: true }) };
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
          // New lengths change every run's minutes and the keyboard step at once.
          onrunlengths: (minutes: number) => {
            store.runStep = minutes;
            void store.refresh();
          },
        };
      case 'boss-knowledge':
        return { selectedKey: params.boss ?? '', difficulty: router.query.get('difficulty') ?? '' };
      case 'extraction':
      case 'chat-interaction':
        return { id: params.id ?? '', timeZone: store.week?.timezone ?? 'Asia/Kuala_Lumpur', toaster };
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
    void tick().then(() => document.getElementById('main')?.focus({ preventScroll: true }));
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
    // The pane lives in the Week window: the palette can open a run from any page.
    if (route?.key !== 'week') router.go(which === 'next' ? '/?week=next' : '/');
    sheetRunId = runId;
    sheetOpen = true;
  }

  // Closing the pane returns focus to the run it came from (a board card or a Runs row).
  function closePane() {
    const id = sheetRunId;
    sheetOpen = false;
    void tick().then(() => {
      const back = id
        ? (document.querySelector<HTMLElement>(`[data-handle="${CSS.escape(id)}"]`) ??
          document.querySelector<HTMLElement>(`[data-row="${CSS.escape(id)}"] .week-runs__open`))
        : null;
      (back ?? document.getElementById('main'))?.focus({ preventScroll: true });
    });
  }

  function report(outcome: MoveOutcome, undo?: () => void) {
    return toaster.show({
      message: outcome.message,
      tone: outcome.ok ? 'ok' : 'error',
      // Ten seconds, paused on hover/focus; Ctrl/Cmd+Z and the palette also undo moves.
      timeoutMs: outcome.ok ? 10_000 : null,
      action: outcome.ok && undo ? { label: 'Undo', run: undo } : undefined,
    });
  }

  // A toast represents one undo entry, not "whatever changed most recently".
  // Retire its button when a later planner change earns the single undo slot.
  let plannerUndoToast: number | null = null;
  function reportPlanner(outcome: MoveOutcome) {
    if (!outcome.ok) return report(outcome);
    const entry = store.lastMove;
    if (!entry) return report(outcome);
    if (plannerUndoToast !== null) toaster.dismiss(plannerUndoToast);
    // The toaster can keep an older item through a route/render boundary; no
    // actionable toast may outlive the one planner undo entry.
    toaster.items.filter((toast) => toast.action?.label === 'Undo').forEach((toast) => toaster.dismiss(toast.id));
    plannerUndoToast = report(outcome, () => void undo(entry.revision));
  }

  async function move(runId: string, to: Slot): Promise<MoveOutcome> {
    const outcome = await store.move(runId, to);
    reportPlanner(outcome);
    return outcome;
  }

  async function swap(runId: string, withId: string): Promise<MoveOutcome> {
    const outcome = await store.swap(runId, withId);
    reportPlanner(outcome);
    return outcome;
  }

  async function undo(revision?: number) {
    const runId = store.lastMove?.runId;
    const prior = document.activeElement;
    const outcome = await store.undo(revision);
    if (!outcome) return;
    report(outcome);
    // Undo from a toast removes the button that had focus; land on the moved
    // card's handle instead of the document body, as does the page-head button,
    // which disables itself. Ctrl/Cmd-Z elsewhere keeps focus where it was.
    await tick();
    // A dismissed toast stays a moment for its exit, inert: its button no longer counts.
    const kept = prior instanceof HTMLElement && prior.isConnected && prior !== document.body && !prior.matches(':disabled') && !prior.closest('[inert]');
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
    ...(store.lastMove ? [{ id: 'undo', label: store.lastMove.kind === 'swap' ? 'Undo last swap' : 'Undo last move', group: 'Edit', keywords: 'revert', run: () => void undo() }] : []),
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
      // The stored key too, so older names (twilight, blossom…) still match.
      keywords: `theme colour color ${way.key}`,
      run: () => applyColorway(way.key),
    })),
    ...(['system', 'light', 'dark'] as const).map((mode) => ({
      id: `mode-${mode}`,
      label: `Mode: ${mode[0]!.toUpperCase()}${mode.slice(1)}`,
      group: 'Theme',
      keywords: 'theme dark light night',
      run: () => applyMode(mode),
    })),
    {
      id: 'experiments',
      label: experiments.on ? 'Turn design experiments off' : 'Turn design experiments on',
      group: 'Theme',
      keywords: 'experiments loading indicator wavy progress',
      run: () => setExperiments(!experiments.on),
    },
    {
      id: 'overshoot',
      label: experiments.overshootChosen ? 'Turn planner overshoot off' : 'Turn planner overshoot on (experiment E)',
      group: 'Theme',
      keywords: 'experiments overshoot spring bounce planner motion',
      run: () => setOvershoot(!experiments.overshootChosen),
    },
  ]);

  function typing(target: EventTarget | null): boolean {
    return target instanceof HTMLElement && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName));
  }

  function onKeydown(event: KeyboardEvent) {
    const mod = event.metaKey || event.ctrlKey;
    if (mod && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      void togglePalette();
    } else if (mod && event.key.toLowerCase() === 'z' && !event.shiftKey && !typing(event.target) && !paletteOpen && !(sheetOpen && !sheetWide)) {
      if (store.lastMove) {
        event.preventDefault();
        void undo();
      }
    }
  }
</script>

{#snippet sheet(wide: boolean)}
  {#if store.week && RunSheet}
    <RunSheet
      bind:open={sheetOpen}
      run={sheetRun}
      week={store.week}
      members={store.members}
      {wide}
      countdown={store.summary?.next?.run_id === sheetRunId ? store.summary.next.countdown : null}
      onclose={closePane}
      onmove={(runId, to) => move(runId, to)}
      onswap={(runId, withId) => swap(runId, withId)}
      onstatus={(runId, status) => store.setStatus(runId, status)}
      onrsvp={(runId, memberId, answer) => store.rsvp(runId, memberId, answer)}
      onroster={(runId, change) => store.roster(runId, change)}
      onreset={(runId) => store.resetToFixed(runId)}
      onping={(runId) => store.ping(runId)}
      onreread={rereadChannel}
      saving={store.mutating}
    />
  {/if}
{/snippet}

{#snippet runPane()}{@render sheet(true)}{/snippet}

<svelte:window onkeydown={onKeydown} />

{#if route?.key === 'login'}
  <main id="main" tabindex="-1">
    <Lazy loader={PAGES.login} props={pageProps('login', {})} />
  </main>
{:else}
  <div class="frame frame--rail" class:frame--phone={phone}>
    <a class="skip" href="#main">Skip to the page</a>
    {#snippet account()}
      <AccountMenu session={store.session} userId={accountId} oncopy={copyAccountId} onsignout={signOut} />
    {/snippet}
    {#if phone}
      <TopBar
        title={section?.label ?? title}
        open={drawerOpen}
        inbox={store.summary?.inbox ?? 0}
        onInbox={section?.key === 'inbox'}
        fresh={store.fresh}
        updated={store.updated}
        timezone={store.week?.timezone ?? ''}
        drawerId="nav-drawer"
        back={pageBack}
        bind:menu={menuButton}
        onmenu={() => (drawerOpen = true)}
      />
      <NavDrawer
        bind:open={drawerOpen}
        id="nav-drawer"
        name={store.identity?.name ?? 'Kanade'}
        avatar={store.identity ? artUrl(store.identity.avatar, store.identity) : null}
        active={section?.key ?? ''}
        inbox={store.summary?.inbox ?? 0}
        counts={store.drawerCounts}
        timezone={store.week?.timezone ?? ''}
        returnTo={menuButton}
        {account}
      />
      {#if !drawerOpen}<EdgeSwipe onopen={() => (drawerOpen = true)} />{/if}
    {:else}
      <Rail
        name={store.identity?.name ?? 'Kanade'}
        avatar={store.identity ? artUrl(store.identity.avatar, store.identity) : null}
        active={section?.key ?? ''}
        inbox={store.summary?.inbox ?? 0}
        {account}
      />
    {/if}
    <main class="shell" id="main" tabindex="-1">
      {#if route?.key === 'week'}
        <WeekPage
          {store}
          {which}
          bind:tab={weekTab}
          selectedRun={sheetOpen && sheetRun ? sheetRunId : null}
          onmove={(runId, to) => void move(runId, to)}
          onswap={(runId, withId) => void swap(runId, withId)}
          onopen={openSheet}
          onclose={sheetWide ? closePane : undefined}
          onundo={() => void undo()}
          onreread={(run) => void rereadFromBoard(run)}
          busyChannels={rereading}
          pane={sheetWide && RunSheet && store.week ? runPane : undefined}
        />
      {:else if loader && route}
        {#key route.key === 'bosses' || route.key === 'boss-knowledge' ? 'boss-workspace' : `${route.key} ${JSON.stringify(route.params)}`}<Lazy {loader} props={pageProps(route.key, route.params)} />{/key}
      {:else}
        <NotFoundPage path={router.path} />
      {/if}

      <p class="footnote">
        <span>All times {store.week?.timezone ?? 'Asia/Kuala_Lumpur'}.</span>
        <span>Boss week starts {store.week?.reset ?? 'Thu 00:00'}.</span>
        <span>Moves apply to the week on screen only.</span>
      </p>
    </main>
    {#if store.week && RunSheet && !sheetWide}{@render sheet(false)}{/if}
    {#if Palette}<Palette bind:open={paletteOpen} {commands} />{/if}
  </div>
{/if}
<ToastRegion {toaster} />
