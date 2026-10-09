<!--
  The member portal (member-auth-contract §6): status first; closed → Closed
  (sign-in hidden); open → the session: 401 → Sign in, 200 → the member's
  screens. `?login_error=` lands on Denied, Closed or Sign in with a notice.
  Signed in: `/` the Week, `/mine` My runs, `/account` Account; nothing
  member-specific is read before sign-in or kept after it.

  Chrome (boards Mast, PhoneDrawer): wide screens keep the masthead (the
  time zone when signed out; signed in, the Week · My runs tabs, the
  freshness chip, the zone and the account menu); below 900 px (and phone
  landscape) a 48 px top bar and, signed in, the navigation drawer. Requests
  and Bosses join the masthead with their screens.
-->
<script lang="ts">
  import { AccountMenu, Freshness, Icon, LoadingState, Masthead, NavDrawer, PHONE_QUERY, registerServiceWorker, SINGLE_PANE_QUERY, StateNote, ToastRegion, Toaster, type FreshState } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import Account, { type AccountTab } from './Account.svelte';
  import Closed from './Closed.svelte';
  import Denied from './Denied.svelte';
  import Ended from './Ended.svelte';
  import MyRuns from './MyRuns.svelte';
  import SignIn from './SignIn.svelte';
  import TopBar from './TopBar.svelte';
  import WeekPage from './week/WeekPage.svelte';
  import { landingOf, safeNext, withoutLoginError } from './landing';
  import { Portal } from './portal.svelte';
  import { Route, type Page } from './route.svelte';

  const portal = new Portal(landingOf(location.search));
  const route = new Route();
  const toaster = new Toaster();
  // Back to this address after signing in, minus the outcome of the last attempt.
  const here = withoutLoginError(location.pathname, location.search, location.hash);
  const next = safeNext(here);
  if (here !== `${location.pathname}${location.search}${location.hash}`) history.replaceState(history.state, '', here);

  const screen = $derived(portal.screen);
  const member = $derived(screen.kind === 'member' ? screen.session.member : null);
  const identity = $derived(portal.identity);
  const botName = $derived(identity?.name ?? 'Kanade');
  const weeks = portal.weeks;
  // Times are the guild's (the week's zone), named as an offset ("GMT+8"); before a week arrives, this device's.
  const timeZone = $derived(weeks.this?.timezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone);
  const zone = $derived(
    new Intl.DateTimeFormat('en', { timeZone, timeZoneName: 'shortOffset' }).formatToParts(new Date()).find((part) => part.type === 'timeZoneName')?.value ?? timeZone,
  );

  // The phone frame (top bar + drawer) or the masthead; exactly one is rendered.
  // Single-pane widths (below 900 px) and phone landscape use the phone frame.
  const narrowQuery = new MediaQuery(SINGLE_PANE_QUERY);
  const phoneQuery = new MediaQuery(PHONE_QUERY);
  const phone = $derived(narrowQuery.current || phoneQuery.current);
  let drawerOpen = $state(false);
  let menuButton = $state<HTMLButtonElement>();
  $effect(() => {
    if (!phone || !member) drawerOpen = false;
  });

  const page: Page = $derived(route.page);
  // Account's tab lives in the address (`?tab=`), as the admin Account's does.
  const TABS: AccountTab[] = ['profile', 'devices', 'browser'];
  const tab: AccountTab = $derived(TABS.find((t) => t === route.params.get('tab')) ?? 'profile');
  const showTab = (to: AccountTab) => route.set({ tab: to === 'profile' ? '' : to });

  // The run open on a phone is its own screen: the top bar goes back to the Week.
  const phoneRun = $derived.by(() => {
    if (!phone || page !== 'week') return null;
    const id = route.params.get('run');
    const week = weeks.week(route.params.get('week') === 'next' ? 'next' : 'this');
    return id ? (week?.runs.find((r) => r.id === id) ?? null) : null;
  });

  // Offline signed in: the screen stays, with the last-seen week and a notice saying so.
  let online = $state(navigator.onLine);
  $effect(() => {
    const update = () => {
      online = navigator.onLine;
      if (online && member) void portal.refresh();
    };
    addEventListener('online', update);
    addEventListener('offline', update);
    return () => {
      removeEventListener('online', update);
      removeEventListener('offline', update);
    };
  });
  const offline = $derived(!online || weeks.offline);
  // Freshness and "as of" follow the week; before one arrives, the session's read.
  const seen = $derived(weeks.updated ?? portal.updated);
  const updated = $derived(seen === null ? '' : new Date(seen).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }));
  const freshness: FreshState = $derived(offline ? 'offline' : !weeks.this ? 'loading' : 'live');
  const myRuns = $derived(weeks.this ? weeks.this.runs.filter((r) => r.mine).length : null);

  const TITLES: Record<Page, string> = { week: 'Week', mine: 'My runs', account: 'Account' };
  const SCREEN_TITLES: Record<string, string> = {
    loading: 'Kanade',
    closed: 'Closed · Kanade',
    unreachable: 'Kanade',
    signin: 'Sign in · Kanade',
    denied: 'Sign in · Kanade',
    ended: 'Session ended · Kanade',
  };

  $effect(() => portal.watch());
  $effect(() => route.watch());
  // Once, on mount: `load()` reads the screen, which must not make this effect rerun.
  $effect(() =>
    untrack(() => {
      void portal.load();
      void portal.loadIdentity();
    }),
  );

  // The first screen keeps the page's natural focus; every later one (and
  // every page change) moves focus to the new page.
  let main = $state<HTMLElement>();
  let settled = false;
  $effect(() => {
    const kind = screen.kind;
    const now = page;
    document.title = kind === 'member' ? `${TITLES[now]} · Kanade` : (SCREEN_TITLES[kind] ?? 'Kanade');
    if (kind === 'loading') return;
    if (settled) void tick().then(() => main?.focus({ preventScroll: true }));
    settled = true;
  });

  $effect(() => {
    void registerServiceWorker({
      url: '/sw.js',
      onUpdateReady: (apply) =>
        toaster.show({ message: 'A new version of Kanade is ready.', timeoutMs: null, action: { label: 'Reload', run: apply } }),
      onControllerChange: () =>
        toaster.show({
          message: 'Kanade was updated in another tab.',
          timeoutMs: null,
          action: { label: 'Reload', run: () => location.reload() },
        }),
    });
  });

  function go(to: Page, event?: MouseEvent) {
    if (event && (event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0)) return;
    event?.preventDefault();
    route.go(to === 'week' ? '/' : `/${to === 'mine' ? 'mine' : 'account'}`);
  }

  /** The account menu's Account and Appearance: open that tab and focus its heading. */
  async function jump(to: AccountTab, id: string) {
    route.go('/account', to === 'profile' ? {} : { tab: to });
    await tick();
    const target = document.getElementById(id);
    target?.focus({ preventScroll: true });
    target?.scrollIntoView({ block: 'nearest' });
  }

  async function signOut() {
    const message = await portal.signOut();
    if (message) toaster.show({ message, tone: 'error' });
  }

  const NAV: { page: Page; label: string; href: string; icon: 'calendar' | 'clock' | 'users' }[] = [
    { page: 'week', label: 'Week', href: '/', icon: 'calendar' },
    { page: 'mine', label: 'My runs', href: '/mine', icon: 'clock' },
    { page: 'account', label: 'Account', href: '/account', icon: 'users' },
  ];
</script>

{#snippet offlineNotice()}
  {#if offline}
    <p class="flash flash--warn portal-offline" role="status" data-fid="week-offline">
      <Icon name="wifi-off" />
      <span class="portal-offline__words"
        ><b>You're offline.</b>
        {#if page === 'account'}Showing your account as of <span class="mono">{updated || 'sign-in'}</span>. Changes wait until you reconnect.{:else if weeks.this}Showing the week
          as of <span class="mono">{updated}</span>. Answers and moves wait until you reconnect.{:else}Reconnect to see the week.{/if}</span
      >
      <button type="button" class="btn" onclick={() => void portal.refresh()}>Try again</button>
    </p>
  {/if}
{/snippet}

<!-- The week's freshness in the phone top bar; Account's top bar goes without (board PhoneAccount). -->
{#snippet freshChip()}<Freshness state={freshness} {updated} />{/snippet}

<div class="frame" class:frame--phone={phone}>
  <a class="skip" href="#main">Skip to the content</a>
  {#if phone}
    <TopBar
      name={botName}
      avatar={identity?.avatar ?? null}
      {zone}
      {member}
      title={TITLES[page]}
      open={drawerOpen}
      drawerId="portal-drawer"
      bind:menu={menuButton}
      back={phoneRun ? { label: 'Week', chip: phoneRun.mine ? "YOU'RE IN" : 'not in this run · view only', mine: phoneRun.mine, onback: () => route.set({ run: '' }) } : null}
      onmenu={() => (drawerOpen = true)}
      onprofile={() => void jump('profile', 'acct-page')}
      fresh={page === 'account' ? undefined : freshChip}
    />
    {#if member}
      <NavDrawer bind:open={drawerOpen} id="portal-drawer" name={botName} avatar={identity?.avatar ?? null} returnTo={menuButton}>
        {#snippet nav(follow)}
          <nav class="navlist" aria-label="Main" data-fid="drawer-nav">
            {#each NAV as item (item.page)}
              <a
                class="navlist__item"
                href={item.href}
                aria-current={page === item.page ? 'page' : undefined}
                onclick={(event) => {
                  follow(event);
                  go(item.page, event);
                }}
                ><span class="navlist__ind"><Icon name={item.icon} /></span><span class="navlist__label">{item.label}</span>{#if item.page === 'mine' && myRuns !== null}<span
                    class="navlist__badge mono">{myRuns}</span
                  >{/if}</a
              >
            {/each}
          </nav>
        {/snippet}
        {#snippet foot()}
          <div class="drawer__row">
            <span>Times in {zone}</span>
            <button
              type="button"
              class="btn"
              onclick={() => {
                drawerOpen = false;
                void signOut();
              }}><Icon name="log-out" />Sign out</button
            >
          </div>
        {/snippet}
      </NavDrawer>
    {/if}
  {:else}
    <Masthead name={botName} avatar={identity?.avatar ?? null} href="/" by="boss schedule · {location.hostname}">
      {#snippet nav()}
        {#if member}
          <nav class="masthead__nav" aria-label="Main" data-fid="mast-nav">
            {#each NAV.filter((item) => item.page !== 'account') as item (item.page)}
              <a class="ptab masthead__tab" href={item.href} aria-current={page === item.page ? 'page' : undefined} onclick={(event) => go(item.page, event)}
                >{item.label}{#if item.page === 'mine' && myRuns !== null}<span class="ptab__count mono">{myRuns}</span>{/if}</a
              >
            {/each}
          </nav>
        {/if}
      {/snippet}
      {#snippet meta()}
        {#if member}
          <span class="mchip" data-fid="topbar-fresh"><Freshness state={freshness} {updated} /></span>
          <span class="masthead__zone" title="Times are in {timeZone}">{zone}</span>
          <AccountMenu who={member.display} avatar={member.avatar} detail="signed in with Discord" data-fid="mast-account">
            {#snippet items(hide)}
              <button
                type="button"
                role="menuitem"
                tabindex="-1"
                class="account__item"
                onclick={() => {
                  hide(false);
                  void jump('profile', 'acct-page');
                }}><Icon name="users" /> Account</button
              >
              <button
                type="button"
                role="menuitem"
                tabindex="-1"
                class="account__item"
                onclick={() => {
                  hide(false);
                  void jump('browser', 'acct-look');
                }}><Icon name="sliders" /> Appearance</button
              >
              <button
                type="button"
                role="menuitem"
                tabindex="-1"
                class="account__item"
                onclick={() => {
                  hide(false);
                  void signOut();
                }}><Icon name="log-out" /> Sign out</button
              >
            {/snippet}
          </AccountMenu>
        {:else}
          <span class="masthead__zone" title="Times are in {timeZone}">Times in {zone}</span>
        {/if}
      {/snippet}
    </Masthead>
  {/if}
  <main class="shell" class:shell--run={phoneRun !== null} id="main" tabindex="-1" bind:this={main}>
    {#if screen.kind === 'loading'}
      <div class="gate">
        <section class="card notice notice--loading" aria-busy="true" aria-labelledby="loading-title">
          <div class="card__head"><h1 class="card__title" id="loading-title">Loading</h1></div>
          <LoadingState text="Loading…" />
        </section>
      </div>
    {:else if screen.kind === 'closed'}
      <Closed {identity} {zone} {phone} oncheck={() => void portal.load()} />
    {:else if screen.kind === 'unreachable'}
      <div class="gate">
        <section class="card notice" aria-labelledby="unreachable-bar">
          <div class="card__head"><span class="card__title" id="unreachable-bar">{screen.offline ? 'Offline' : 'No answer'}</span></div>
          <StateNote icon={screen.offline ? 'wifi-off' : 'alert-circle'} level={1} title={screen.offline ? "You're offline" : "Kanade can't be reached"}>
            {screen.offline ? 'Reconnect to sign in. Nothing is kept on this device.' : "Kanade didn't answer. Try again in a moment."}
            {#snippet actions()}
              <button type="button" class="btn btn--primary btn--key" onclick={() => void portal.load()}>Try again</button>
            {/snippet}
          </StateNote>
        </section>
      </div>
    {:else if screen.kind === 'signin'}
      <SignIn {identity} notice={screen.notice} {next} {zone} {phone} />
    {:else if screen.kind === 'denied'}
      <Denied {identity} {zone} {phone} {next} onswitch={() => portal.switchAccount()} />
    {:else if screen.kind === 'ended'}
      <Ended {identity} {zone} {phone} {next} at={screen.at} />
    {:else if page === 'week'}
      <WeekPage {weeks} {route} memberId={screen.session.member.id} {phone} {toaster} notice={offlineNotice} />
    {:else if page === 'mine'}
      <MyRuns
        {weeks}
        timings={portal.timings}
        {route}
        session={screen.session}
        current={portal.devices?.sessions.find((s) => s.current) ?? null}
        {toaster}
        {phone}
        notice={offlineNotice}
      />
    {:else}
      <Account {portal} session={screen.session} {toaster} {phone} {zone} {timeZone} {tab} ontab={showTab} notice={offlineNotice} />
    {/if}
  </main>
  <ToastRegion {toaster} />
</div>
