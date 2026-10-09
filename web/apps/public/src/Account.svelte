<!--
  Signed in (boards Account, Account-Devices, Account-Browser; phone
  PhoneAccount, PhoneDevices, PhoneBrowser): the admin Account window
  (`_account.scss`) without its admin parts. Profile / Devices / This browser
  title-bar tabs (`?tab=`); on wide screens the identity stands fixed beside
  the one scrolling panel, on phones it leads the Profile tab. No IP or
  location anywhere: the server never sends them. The rows for features the
  portal does not have yet (time zone, mentions, reply style, chat
  allowance, calendar) are drawn as the boards draw them, marked "coming
  soon", with their controls disabled and no figures.
-->
<script lang="ts" module>
  export type AccountTab = 'profile' | 'devices' | 'browser';
</script>

<script lang="ts">
  import type { PublicSession } from '@kanade/api-types';
  import { Avatar, dayTime, Icon, motionPreference, SessionList, SwitchRow, ThemeTiles, type IconName, type Toaster } from '@kanade/ui';
  import { tick, type Snippet } from 'svelte';
  import type { Portal } from './portal.svelte';

  let {
    portal,
    session,
    toaster,
    phone,
    zone,
    timeZone,
    tab,
    ontab,
    notice,
  }: {
    portal: Portal;
    session: PublicSession;
    toaster: Toaster;
    phone: boolean;
    /** The device's zone as an offset ("GMT+8"). */
    zone: string;
    /** The device's IANA zone, for times. */
    timeZone: string;
    tab: AccountTab;
    ontab: (tab: AccountTab) => void;
    /** A notice about the whole screen (offline), under the page line or the title bar. */
    notice?: Snippet;
  } = $props();

  const TABS: { id: AccountTab; label: string }[] = [
    { id: 'profile', label: 'Profile' },
    { id: 'devices', label: 'Devices' },
    { id: 'browser', label: 'This browser' },
  ];
  // The server keeps at most ten sessions per member (D5-A): an 11th sign-in ends the oldest.
  const SESSION_LIMIT = 10;

  const member = $derived(session.member);
  // This device first, then the newest sign-in (the server answers oldest first).
  const rows = $derived(
    portal.devices
      ? [...portal.devices.sessions].sort((a, b) => Number(b.current) - Number(a.current) || Date.parse(b.signed_in_at) - Date.parse(a.signed_in_at))
      : null,
  );
  const signedIn = $derived(rows?.find((row) => row.current)?.signed_in_at ?? null);

  const tabs: Partial<Record<AccountTab, HTMLButtonElement>> = {};
  function choose(next: AccountTab, focus = false) {
    ontab(next);
    // Devices re-reads each time it opens: another device may have signed in or out.
    if (next === 'devices') void portal.loadDevices();
    if (focus) void tick().then(() => tabs[next]?.focus());
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const to = moves[event.key];
    if (to === undefined) return;
    event.preventDefault();
    choose(TABS[(to + TABS.length) % TABS.length]!.id, true);
  }

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

  type Soon = { icon: IconName; title: string; sub: string; control: 'change' | 'switch' | 'copy' };
  const zoneRow: Soon = $derived({ icon: 'clock', title: 'Time zone', sub: `${zone} · taken from this browser for now`, control: 'change' });
  const MENTIONS: Soon = { icon: 'bell', title: '@mentions', sub: 'When one of your runs moves, changes party or is at risk', control: 'switch' };
  const STYLE: Soon = { icon: 'message-square', title: 'Reply style', sub: 'How Kanade talks to you in chat; schedule facts stay the same', control: 'change' };
  const CALENDAR: Soon = { icon: 'calendar', title: 'Calendar feed', sub: 'Your runs in Google or Apple Calendar, as a private link you can revoke', control: 'copy' };
</script>

{#snippet soonChip()}<span class="status-chip status-chip--warn">coming soon</span>{/snippet}

{#snippet access()}
  <section class="account-sec" aria-labelledby="acct-access">
    <div class="account-sec__head">
      <h3 class="cap" id="acct-access">Access</h3>
      {#if !phone}<span class="account-sec__end account-sec__note">Checked at sign-in: <b>member</b></span>{/if}
    </div>
    <ul class="account-grp">
      <li class="account-row" class:account-row--tight={phone}>
        <span class="account-lead account-lead--ok"><Icon name="check" /></span>
        <span class="account-row__text">
          <span class="account-row__title">Bossing role</span>
          <span class="account-row__sub">You can answer and move your own runs</span>
        </span>
        <span class="status-chip status-chip--ok"><Icon name="check" />Yes</span>
      </li>
    </ul>
  </section>
{/snippet}

{#snippet allowanceBody()}
  <span class="account-lead account-lead--accent"><Icon name="gauge" /></span>
  <span class="account-row__text">
    <span class="account-row__title">Chat allowance</span>
    <span class="account-row__sub">Answers Kanade gives you in chat</span>
  </span>
  {@render soonChip()}
{/snippet}

<!-- The phone boards tag neither the allowance row nor the note. -->
{#snippet allowance()}
  {#if phone}
    <li class="account-row account-row--soon account-row--tight">{@render allowanceBody()}</li>
  {:else}
    <li class="account-row account-row--soon" data-fid="account-allowance">{@render allowanceBody()}</li>
  {/if}
{/snippet}

{#snippet soonRow(row: Soon)}
  <li class="account-row account-row--soon" class:account-row--tight={phone}>
    <span class="account-lead"><Icon name={row.icon} /></span>
    <span class="account-row__text">
      <span class="account-row__title">{row.title}</span>
      <span class="account-row__sub">{row.sub}</span>
    </span>
    {@render soonChip()}
    {#if !phone}
      {#if row.control === 'switch'}
        <button type="button" class="switch" role="switch" aria-checked="false" aria-disabled="true" aria-label={row.title}
          ><span class="switch__knob" aria-hidden="true"></span></button
        >
      {:else}
        <button type="button" class="btn" aria-disabled="true">{row.control === 'copy' ? 'Copy link' : 'Change…'}</button>
      {/if}
    {/if}
  </li>
{/snippet}

{#snippet profile()}
  <div class="account-col">
    {#if phone}
      <div class="account-row account-me" data-fid="account-id">
        <Avatar class="account-me__portrait" src={member.avatar} name={member.display} />
        <span class="account-row__text">
          <span class="cap">Signed in as</span>
          <span class="account-me__name">{member.display}</span>
          <span class="account-row__sub">with Discord · bossing role</span>
        </span>
        <button type="button" class="btn account-full" onclick={() => void report(portal.signOut())} aria-disabled={portal.busy !== ''}>
          <Icon name="log-out" />{portal.busy === 'self' ? 'Signing out…' : 'Sign out on this device'}
        </button>
      </div>
    {/if}
    {@render access()}
    <section class="account-sec" aria-labelledby="acct-reach" data-fid="account-reach">
      <div class="account-sec__head">
        <h3 class="cap" id="acct-reach">How Kanade reaches you</h3>
        {#if !phone}<span class="account-sec__end account-sec__note">These come with the next portal update</span>{/if}
      </div>
      <ul class="account-grp">
        {@render soonRow(zoneRow)}
        {@render soonRow(MENTIONS)}
        {#if !phone}
          {@render soonRow(STYLE)}
          {@render allowance()}
        {/if}
        {@render soonRow(CALENDAR)}
      </ul>
    </section>
    {#if phone}
      <section class="account-sec" aria-labelledby="acct-chat" data-fid="account-chat">
        <div class="account-sec__head"><h3 class="cap" id="acct-chat">Chat with Kanade</h3></div>
        <ul class="account-grp">
          {@render allowance()}
          {@render soonRow(STYLE)}
        </ul>
      </section>
    {/if}
    {#if phone}
      <p class="infobox"><Icon name="info" /><span>Kanade keeps your Discord id, display name and avatar. Nothing else.</span></p>
    {:else}
      <p class="infobox" data-fid="account-note">
        <Icon name="info" />
        <span>Kanade keeps your Discord id, display name and avatar, and your answers and requests. Nothing else.</span>
      </p>
    {/if}
  </div>
{/snippet}

{#snippet devices()}
  <SessionList
    {rows}
    now={portal.devices?.generated_at ?? ''}
    error={portal.devicesError}
    onretry={() => void portal.loadDevices()}
    {timeZone}
    compact={phone}
    busy={portal.busy}
    onend={(handle, name) => void endDevice(handle, name)}
    endAll={{ label: portal.busy === 'everywhere' ? 'Signing out…' : 'Sign out everywhere', run: () => void report(portal.endEverywhere()) }}
    title="Signed-in devices"
    titleId="acct-devices"
    focusableTitle
    thing="your devices"
    loading="Loading your devices…"
    current="This device"
    currentHint="Use Sign out on the left"
    limit={SESSION_LIMIT}
  >
    {#snippet note()}
      <p class="infobox">
        <Icon name="info" />
        <span>Signing a device out ends its session at once. Sign out everywhere ends every session, this one too. An 11th sign-in ends the oldest.</span>
      </p>
    {/snippet}
  </SessionList>
{/snippet}

{#snippet kept()}
  <p class="infobox"><Icon name="monitor" /><span>Kept in this browser only; nothing is sent to the server. Your other devices keep their own choice.</span></p>
{/snippet}

{#snippet browser()}
  <div class="account-col">
    {#if phone}{@render kept()}{/if}
    <section class="account-sec" aria-labelledby="acct-look">
      <div class="account-sec__head">
        <h3 class="cap" id="acct-look" tabindex="-1">Appearance</h3>
        {#if !phone}<span class="account-sec__end account-sec__note">Kept in this browser only</span>{/if}
      </div>
      <ThemeTiles />
    </section>
    <section class="account-sec" aria-labelledby="acct-motion" data-fid="account-motion">
      <div class="account-sec__head"><h3 class="cap" id="acct-motion">Motion</h3></div>
      <div class="account-grp">
        <SwitchRow
          id="acct-reduce"
          icon={phone ? undefined : 'sliders'}
          title="Reduce motion"
          note="Still shapes and flat bars, even if your system allows motion"
          on={motionPreference.reduce}
          onflip={() => motionPreference.set(!motionPreference.reduce)}
        />
      </div>
    </section>
    {#if !phone}{@render kept()}{/if}
  </div>
{/snippet}

{#snippet strip()}
  <div class="card__head tabs__strip" class:phone-tabs={phone} data-fid="window-bar">
    <h2 class="vh" id="acct-title">Account</h2>
    <div class="tabs__tabs" role="tablist" aria-label="Account" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        <button
          class="tabs__tab"
          role="tab"
          type="button"
          id="acct-tab-{t.id}"
          aria-selected={tab === t.id}
          aria-controls="acct-panel"
          tabindex={tab === t.id ? 0 : -1}
          bind:this={tabs[t.id]}
          onclick={() => choose(t.id)}
          onkeydown={(event) => tabKey(event, index)}
          >{t.label}{#if t.id === 'devices' && rows}<span class="tabs__count">{rows.length}</span>{/if}</button
        >
      {/each}
    </div>
  </div>
{/snippet}

{#snippet panel()}
  {#if tab === 'devices'}{@render devices()}{:else if tab === 'browser'}{@render browser()}{:else}{@render profile()}{/if}
{/snippet}

{#if phone}
  <h1 class="vh" id="acct-page" tabindex="-1">Account</h1>
  <section class="card tabs window-fill account-window" aria-labelledby="acct-title" data-fid="window">
    {@render strip()}
    {@render notice?.()}
    <div
      class="account-window__panel account-window__panel--phone"
      class:account-window__panel--flush={tab === 'devices'}
      id="acct-panel"
      role="tabpanel"
      aria-labelledby="acct-tab-{tab}"
      tabindex="0"
    >
      {@render panel()}
    </div>
  </section>
{:else}
  <div class="pageline" data-fid="page-line">
    <div class="pageline__head">
      <h1 class="pageline__title" id="acct-page" tabindex="-1">Account</h1>
      <p class="pageline__context">· {member.display} · signed in with Discord</p>
    </div>
  </div>
  {@render notice?.()}
  <section class="card tabs window-fill account-window" aria-labelledby="acct-title" data-fid="window">
    {@render strip()}
    <div class="account-window__body">
      <aside class="account-id" aria-label="You" data-fid="account-id">
        <div class="account-id__who">
          <Avatar class="account-id__portrait" src={member.avatar} name={member.display} />
          <span class="cap">Discord member</span>
          <h2 class="account-id__name" title={member.display}>{member.display}</h2>
          <span class="account-id__method"><Icon name="users" />Bossing role · signed in with Discord</span>
        </div>
        <section class="account-id__diag" aria-labelledby="acct-you">
          <h3 class="cap" id="acct-you">You</h3>
          <dl class="account-id__dl">
            <dt>Discord id</dt>
            <dd>{member.id}</dd>
            <dt>Signed in</dt>
            <dd>{signedIn ? dayTime(signedIn, timeZone) : '…'}</dd>
            <dt>Times</dt>
            <dd>{zone}</dd>
          </dl>
        </section>
        <div class="account-id__foot">
          <button type="button" class="btn btn--danger account-full" onclick={() => void report(portal.signOut())} aria-disabled={portal.busy !== ''}>
            <Icon name="log-out" />{portal.busy === 'self' ? 'Signing out…' : 'Sign out'}
          </button>
        </div>
      </aside>
      <div class="account-window__panel" id="acct-panel" role="tabpanel" aria-labelledby="acct-tab-{tab}" tabindex="0" data-fid="account-panel">
        {@render panel()}
      </div>
    </div>
  </section>
{/if}
