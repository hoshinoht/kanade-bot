<!--
  Signed in, this step (board `P_Account`, D5-A): the page line, then the "You"
  window holding cards on the board's surface: the profile (Discord avatar
  and display name, Sign out), the signed-in devices (the shared SessionList:
  this one marked; sign out one; Sign out everywhere, this one too) and
  Appearance (the shared ThemePicker). No IP or location anywhere: the server
  never sends them. Allowance, calendar feed and preferences come in later
  steps, so they are not drawn.
-->
<script lang="ts">
  import '@kanade/ui/styles/account.scss';
  import type { PublicSession } from '@kanade/api-types';
  import { Avatar, Icon, SessionList, ThemePicker, type Toaster } from '@kanade/ui';
  import { tick } from 'svelte';
  import type { Portal } from './portal.svelte';

  let { portal, session, toaster }: { portal: Portal; session: PublicSession; toaster: Toaster } = $props();

  // The member's own clock: device times read in this device's zone.
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  // This device first, then the newest sign-in (the server answers oldest first).
  const rows = $derived(
    portal.devices
      ? [...portal.devices.sessions].sort((a, b) => Number(b.current) - Number(a.current) || Date.parse(b.signed_in_at) - Date.parse(a.signed_in_at))
      : null,
  );

  async function endDevice(handle: string, name: string) {
    const told = await portal.endDevice(handle, name);
    if (!told) return;
    toaster.show({ message: told.message, tone: told.ok ? 'ok' : 'error' });
    // Its button is gone: focus the list's heading rather than the page.
    await tick();
    document.getElementById('acct-devices')?.focus({ preventScroll: true });
  }

  async function report(failed: Promise<string | null>) {
    const message = await failed;
    if (message) toaster.show({ message, tone: 'error' });
  }
</script>

<div class="acct-line">
  <h1 class="acct-line__title" id="acct-page" tabindex="-1">Account</h1>
  <p class="acct-line__sub">signed in with Discord</p>
</div>

<section class="card window-fill acct" aria-labelledby="acct-title">
  <div class="card__head"><h2 class="card__title" id="acct-title">You</h2></div>
  <div class="acct__grid">
    <section class="acct__card acct__profile" aria-label="Profile">
      <Avatar class="acct__portrait" src={session.member.avatar} name={session.member.display} />
      <p class="acct__name">{session.member.display}</p>
      <button type="button" class="btn acct__signout" onclick={() => void report(portal.signOut())} aria-disabled={portal.busy !== ''}>
        {portal.busy === 'self' ? 'Signing out…' : 'Sign out'}
      </button>
    </section>

    <section class="acct__card" aria-labelledby="acct-devices">
      <SessionList
        {rows}
        now={portal.devices?.generated_at ?? ''}
        error={portal.devicesError}
        onretry={() => void portal.loadDevices()}
        timeZone={zone}
        busy={portal.busy}
        onend={(handle, name) => void endDevice(handle, name)}
        endAll={{ label: portal.busy === 'everywhere' ? 'Signing out…' : 'Sign out everywhere', run: () => void report(portal.endEverywhere()) }}
        title="Signed-in devices"
        titleId="acct-devices"
        focusableTitle
        thing="your devices"
        loading="Loading your devices…"
        current="This device"
      >
        {#snippet note()}
          <p class="infobox">
            <Icon name="info" />
            <span>Signing a device out ends its session at once. Sign out everywhere ends every session, this one too.</span>
          </p>
        {/snippet}
      </SessionList>
    </section>

    <section class="acct__card acct__wide" aria-labelledby="acct-look">
      <h3 class="acct__title" id="acct-look" tabindex="-1">Appearance</h3>
      <ThemePicker />
    </section>
  </div>
</section>

<style>
  /* The page line on the ground (P_Account `.top`, 36 px): title and how. */
  .acct-line {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0 12px;
    min-height: 36px;
    margin-bottom: 10px;
    padding: 0 4px;
    color: var(--ground-ink);
  }

  .acct-line__title {
    margin: 0;
    font-family: var(--display);
    font-size: var(--fs-brand);
    font-weight: 800;
  }

  .acct-line__sub {
    margin: 0;
    font-size: var(--fs-small);
  }

  .acct-line__title:focus,
  .acct__title:focus {
    outline: none;
  }

  .acct-line__title:focus-visible,
  .acct__title:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* One window, edge to edge: the 48 px title bar over the board's surface. */
  .acct {
    display: flex;
    flex-direction: column;
    padding: 0;
    border-radius: var(--r-win);
    overflow: clip;
  }

  .acct > :global(.card__head) {
    flex: none;
    min-height: 48px;
    margin: 0;
    padding: 0 16px;
    border-radius: var(--r-win-in) var(--r-win-in) 0 0;
  }

  .acct > :global(.card__head) :global(.card__title) {
    font-size: var(--fs-body);
  }

  /* The one scrolling region: the cards two abreast where there is room. */
  .acct__grid {
    flex: 1 1 auto;
    min-height: 0;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    align-content: start;
    align-items: start;
    gap: 14px;
    padding: 16px;
    background: var(--board);
    overflow: clip auto;
    overscroll-behavior: contain;
  }

  .acct__card {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
    padding: 16px 18px;
    border-radius: var(--r-win);
    background: var(--row);
    box-shadow: inset 0 0 0 1.5px var(--line);
  }

  .acct__wide {
    grid-column: 1 / -1;
  }

  .acct__profile {
    flex-direction: row;
    flex-wrap: wrap;
    align-items: center;
  }

  .acct__profile :global(.acct__portrait) {
    display: grid;
    flex: none;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    background: var(--win);
    color: var(--win-ink);
    font-family: var(--display);
    font-size: var(--fs-lg);
    font-weight: 700;
  }

  .acct__name {
    flex: 1 1 8rem;
    min-width: 0;
    margin: 0;
    font-family: var(--display);
    font-size: var(--fs-lg);
    font-weight: 800;
    overflow-wrap: anywhere;
  }

  .acct__title,
  .acct__card :global(.account-sec__title) {
    margin: 0;
    font-family: var(--body);
    font-size: var(--fs-body);
    font-weight: 700;
  }

  .acct__card :global(.account-col) {
    gap: 10px;
    max-width: none;
  }

  .acct__card :global(.account-sec__head) {
    padding: 0;
  }

  .acct__card :global(.account-chip-acc) {
    margin-left: 8px;
    font-family: var(--mono);
    font-size: var(--fs-micro);
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    vertical-align: middle;
  }

  @media (max-width: 760px) {
    .acct__grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 10px;
      padding: 10px;
    }

    .acct__card {
      padding: 14px;
    }
  }

  /* Phones: the Sign out button drops under the device's text. */
  @media (max-width: 480px) {
    .acct__card :global(.account-session) {
      display: grid;
      grid-template-columns: auto minmax(0, 1fr);
      align-items: center;
    }

    .acct__card :global(.account-session .btn) {
      grid-column: 2;
      justify-self: start;
      min-height: 44px;
    }
  }
</style>
