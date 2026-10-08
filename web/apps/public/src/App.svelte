<!--
  The member portal (member-auth-contract §6): status first; closed → Closed
  (sign-in hidden); open → the session: 401 → Sign in, 200 → masthead and
  Account. `?login_error=` lands on Denied, Closed or Sign in with a notice.
  No schedule is read before or after sign-in in this step. The masthead
  (P_ boards "Shell") names the time zone when signed out and carries the
  account menu (Account, Appearance, Sign out) when signed in.
-->
<script lang="ts">
  import { AccountMenu, Icon, LoadingState, Masthead, registerServiceWorker, StateNote, ToastRegion, Toaster } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import Account from './Account.svelte';
  import Denied from './Denied.svelte';
  import Ended from './Ended.svelte';
  import SignIn from './SignIn.svelte';
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
  // Signed out, the masthead names the zone the portal shows times in: this device's.
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;

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

  /** The account menu's Account and Appearance: the one signed-in screen holds both, so focus that heading. */
  async function jump(id: string) {
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

<div class="frame">
  <a class="skip" href="#main">Skip to the content</a>
  <Masthead name={identity?.name ?? 'Kanade'} avatar={identity?.avatar ?? null} by="boss schedule · {location.hostname}">
    {#snippet meta()}
      {#if member}
        <AccountMenu who={member.display} avatar={member.avatar} detail="signed in with Discord">
          {#snippet items(hide)}
            <button
              type="button"
              role="menuitem"
              tabindex="-1"
              class="account__item"
              onclick={() => {
                hide(false);
                void jump('acct-page');
              }}><Icon name="users" /> Account</button
            >
            <button
              type="button"
              role="menuitem"
              tabindex="-1"
              class="account__item"
              onclick={() => {
                hide(false);
                void jump('acct-look');
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
        <span class="masthead__zone"><span class="vh">Time zone: </span>{zone}</span>
      {/if}
    {/snippet}
  </Masthead>
  <main class="shell" id="main" tabindex="-1" bind:this={main}>
    {#if screen.kind === 'loading'}
      <div class="gate">
        <section class="card notice notice--loading" aria-busy="true" aria-labelledby="loading-title">
          <div class="card__head"><h1 class="card__title" id="loading-title">Loading</h1></div>
          <LoadingState text="Loading…" />
        </section>
      </div>
    {:else if screen.kind === 'closed'}
      <div class="gate">
        <section class="card notice" aria-labelledby="closed-bar">
          <div class="card__head"><span class="card__title" id="closed-bar">Closed</span></div>
          <StateNote icon="moon" level={1} title="The schedule isn't open right now">
            The guild's admins have closed the portal. Reminders and answers still work in Discord.
            {#snippet actions()}
              <button type="button" class="btn btn--key" onclick={() => void portal.load()}>Check again</button>
            {/snippet}
          </StateNote>
        </section>
      </div>
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
      <SignIn {identity} notice={screen.notice} {next} />
    {:else if screen.kind === 'denied'}
      <Denied {next} onswitch={() => portal.switchAccount()} />
    {:else if screen.kind === 'ended'}
      <Ended {next} />
    {:else}
      <Account {portal} session={screen.session} {toaster} />
    {/if}
  </main>
  <ToastRegion {toaster} />
</div>
