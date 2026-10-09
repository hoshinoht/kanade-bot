<!--
  The member portal (member-auth-contract §6): status first; closed → Closed
  (sign-in hidden); open → the session: 401 → Sign in, 200 → Account.
  `?login_error=` lands on Denied, Closed or Sign in with a notice. No
  schedule is read before or after sign-in in this step.

  Chrome (boards Mast, PhoneDrawer): wide screens keep the masthead (the
  time zone when signed out; signed in, the freshness chip, the zone and the
  account menu); phones a 48 px top bar and, signed in, the navigation
  drawer. The masthead's Week · My runs · Requests · Bosses tabs arrive with
  those screens.
-->
<script lang="ts">
  import { AccountMenu, Freshness, Icon, LoadingState, Masthead, NavDrawer, PHONE_QUERY, registerServiceWorker, StateNote, ToastRegion, Toaster, type FreshState } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import Account, { type AccountTab } from './Account.svelte';
  import Closed from './Closed.svelte';
  import Denied from './Denied.svelte';
  import Ended from './Ended.svelte';
  import SignIn from './SignIn.svelte';
  import TopBar from './TopBar.svelte';
  import { landingOf, safeNext, withoutLoginError } from './landing';
  import { Portal } from './portal.svelte';

  const portal = new Portal(landingOf(location.search));
  const toaster = new Toaster();
  // Back to this address after signing in, minus the outcome of the last attempt.
  const here = withoutLoginError(location.pathname, location.search, location.hash);
  const next = safeNext(here);
  if (here !== `${location.pathname}${location.search}${location.hash}`) history.replaceState(history.state, '', here);

  const screen = $derived(portal.screen);
  const member = $derived(screen.kind === 'account' ? screen.session.member : null);
  const identity = $derived(portal.identity);
  const botName = $derived(identity?.name ?? 'Kanade');
  // The portal shows times in this device's zone; the chrome names it as an offset ("GMT+8").
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const zone = new Intl.DateTimeFormat('en', { timeZoneName: 'shortOffset' }).formatToParts(new Date()).find((part) => part.type === 'timeZoneName')?.value ?? timeZone;

  // The phone frame (top bar + drawer) or the masthead; exactly one is rendered.
  const phoneQuery = new MediaQuery(PHONE_QUERY);
  const phone = $derived(phoneQuery.current);
  let drawerOpen = $state(false);
  let menuButton = $state<HTMLButtonElement>();
  $effect(() => {
    if (!phone || !member) drawerOpen = false;
  });

  // Account's tab lives in the address (`?tab=`), as the admin Account's does.
  const TABS: AccountTab[] = ['profile', 'devices', 'browser'];
  const asked = new URLSearchParams(location.search).get('tab');
  let tab = $state<AccountTab>(TABS.find((t) => t === asked) ?? 'profile');
  function showTab(next: AccountTab) {
    tab = next;
    const url = new URL(location.href);
    if (next === 'profile') url.searchParams.delete('tab');
    else url.searchParams.set('tab', next);
    history.replaceState(history.state, '', url);
  }

  // Offline signed in: the account stays, with a notice and the chip saying so.
  let online = $state(navigator.onLine);
  $effect(() => {
    const update = () => (online = navigator.onLine);
    addEventListener('online', update);
    addEventListener('offline', update);
    return () => {
      removeEventListener('online', update);
      removeEventListener('offline', update);
    };
  });
  const updated = $derived(portal.updated === null ? '' : new Date(portal.updated).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }));
  const fresh: FreshState = $derived(!online ? 'offline' : !portal.devices && !portal.devicesError ? 'loading' : 'live');

  const TITLES: Record<string, string> = {
    loading: 'Kanade',
    closed: 'Closed · Kanade',
    unreachable: 'Kanade',
    signin: 'Sign in · Kanade',
    denied: 'Sign in · Kanade',
    ended: 'Session ended · Kanade',
    account: 'Account · Kanade',
  };

  $effect(() => portal.watch());
  // Once, on mount: `load()` reads the screen, which must not make this effect rerun.
  $effect(() =>
    untrack(() => {
      void portal.load();
      void portal.loadIdentity();
    }),
  );

  // The first screen keeps the page's natural focus; every later one moves
  // focus to the new page (the masthead and skip link stay where they are).
  let main = $state<HTMLElement>();
  let settled = false;
  $effect(() => {
    const kind = screen.kind;
    document.title = TITLES[kind] ?? 'Kanade';
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

  /** The account menu's Account and Appearance: open that tab and focus its heading. */
  async function jump(to: AccountTab, id: string) {
    showTab(to);
    await tick();
    const target = document.getElementById(id);
    target?.focus({ preventScroll: true });
    target?.scrollIntoView({ block: 'nearest' });
  }

  async function signOut() {
    const message = await portal.signOut();
    if (message) toaster.show({ message, tone: 'error' });
  }
</script>

{#snippet offline()}
  {#if !online}
    <p class="flash flash--warn portal-offline" role="status" data-fid="week-offline">
      <Icon name="wifi-off" />
      <span class="portal-offline__words"
        ><b>You're offline.</b> Showing your account as of <span class="mono">{updated || 'sign-in'}</span>. Changes wait until you reconnect.</span
      >
      <button type="button" class="btn" onclick={() => void portal.refresh()}>Try again</button>
    </p>
  {/if}
{/snippet}

<div class="frame" class:frame--phone={phone}>
  <a class="skip" href="#main">Skip to the content</a>
  {#if phone}
    <TopBar
      name={botName}
      avatar={identity?.avatar ?? null}
      {zone}
      {member}
      title="Account"
      open={drawerOpen}
      drawerId="portal-drawer"
      bind:menu={menuButton}
      onmenu={() => (drawerOpen = true)}
      onprofile={() => void jump('profile', 'acct-page')}
    />
    {#if member}
      <NavDrawer bind:open={drawerOpen} id="portal-drawer" name={botName} avatar={identity?.avatar ?? null} returnTo={menuButton}>
        {#snippet nav(follow)}
          <nav class="navlist" aria-label="Main" data-fid="drawer-nav">
            <a
              class="navlist__item"
              href="/"
              aria-current="page"
              onclick={(event) => {
                event.preventDefault();
                follow(event);
                void jump('profile', 'acct-page');
              }}><span class="navlist__ind"><Icon name="users" /></span><span class="navlist__label">Account</span></a
            >
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
      {#snippet meta()}
        {#if member}
          <span class="mchip" data-fid="topbar-fresh"><Freshness state={fresh} {updated} /></span>
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
  <main class="shell" id="main" tabindex="-1" bind:this={main}>
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
    {:else}
      <Account {portal} session={screen.session} {toaster} {phone} {zone} {timeZone} {tab} ontab={showTab} notice={offline} />
    {/if}
  </main>
  <ToastRegion {toaster} />
</div>
