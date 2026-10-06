<!--
  Account (boards AdminAccount / AdminPhone, direction B, approved
  2026-10-05): one window with Profile / Sessions / This browser title-bar
  tabs (`?tab=`), the identity fixed beside the one scrolling panel (above it
  on phones). Reads `GET /api/admin/me` and `GET /api/admin/me/sessions`;
  the reply style saves through the Members edit of the admin's own row.
-->
<script lang="ts">
  import '@kanade/ui/styles/settings.scss';
  import '@kanade/ui/styles/account.scss';
  import type { AccountSessions, Me, MemberRow, Persona } from '@kanade/api-types';
  import { LoadError, LoadingState, type Toaster } from '@kanade/ui';
  import { tick } from 'svelte';
  import PageLine from '../shell/PageLine.svelte';
  import { getChrome } from '../shell/chrome';
  import { Resource, send } from '../resource.svelte';
  import { copyText } from '../shared/copy';
  import BrowserTab from './BrowserTab.svelte';
  import Identity from './Identity.svelte';
  import ProfileTab from './ProfileTab.svelte';
  import ReplyPicker from './ReplyPicker.svelte';
  import SessionsTab from './SessionsTab.svelte';
  import { TABS, diagnostics, methodLong, tabOf, type AccountTab } from './account';
  import { serverNow } from '../reminders/when';

  let {
    toaster,
    timeZone,
    tab: asked = '',
    ontab,
    onsignout,
  }: {
    toaster: Toaster;
    /** The guild's zone (`Week.timezone`). */
    timeZone: string;
    tab?: string;
    ontab: (tab: AccountTab) => void;
    onsignout: () => void;
  } = $props();

  const chrome = getChrome();
  const compact = $derived(chrome?.phone ?? false);
  const tab = $derived(tabOf(asked));

  const me = new Resource<Me>('/api/admin/me', { topics: ['members', 'settings'] });
  const sessions = new Resource<AccountSessions>('/api/admin/me/sessions');
  const personas = new Resource<Persona[]>('/api/admin/personas', { topics: ['settings'] });
  $effect(() => {
    const stop = me.watch();
    void sessions.load();
    return stop;
  });

  const tabs: Record<string, HTMLButtonElement> = {};
  function choose(next: AccountTab, focus = false) {
    ontab(next);
    if (next === 'sessions') void sessions.refresh();
    if (focus) void tick().then(() => tabs[next]?.focus());
  }
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const to = moves[event.key];
    if (to === undefined) return;
    event.preventDefault();
    choose(TABS[(to + TABS.length) % TABS.length]!.id, true);
  }

  async function copyDiagnostics() {
    if (!me.data) return;
    const ok = await copyText(diagnostics(me.data, timeZone));
    toaster.show(ok ? { message: 'Copied the diagnostics.', tone: 'ok' } : { message: "Couldn't copy the diagnostics.", tone: 'error' });
  }

  let checking = $state(false);
  async function recheck() {
    if (checking) return;
    checking = true;
    const failed = await me.refresh();
    checking = false;
    if (failed) toaster.show({ message: `Couldn't recheck your access: ${failed}`, tone: 'error' });
  }

  // Sessions: sign one out, or every other one.
  let busy = $state('');
  async function endSession(handle: string, device: string) {
    if (busy) return;
    busy = handle;
    const outcome = await send((client) => client.delete(`/api/admin/me/sessions/${encodeURIComponent(handle)}`));
    busy = '';
    if (outcome.ok || outcome.status === 404) toaster.show({ message: outcome.ok ? `Signed out ${device}.` : 'That session had already ended.', tone: 'ok' });
    else toaster.show({ message: `Couldn't sign out ${device}: ${outcome.message}`, tone: 'error' });
    await sessions.refresh();
    // Its button is gone: focus the list's heading region rather than the page.
    void tick().then(() => document.querySelector<HTMLElement>('.account-window__panel')?.focus({ preventScroll: true }));
  }
  async function endOthers() {
    if (busy) return;
    busy = 'others';
    const outcome = await send((client) => client.post<{ ended: number }>('/api/admin/me/sessions/sign-out-others', {}));
    busy = '';
    if (outcome.ok) {
      const n = outcome.value.ended;
      toaster.show({ message: n ? `Signed out ${n} other session${n === 1 ? '' : 's'}.` : 'No other sessions to sign out.', tone: 'ok' });
    } else toaster.show({ message: `Couldn't sign out the other sessions: ${outcome.message}`, tone: 'error' });
    await sessions.refresh();
    void tick().then(() => document.querySelector<HTMLElement>('.account-window__panel')?.focus({ preventScroll: true }));
  }

  // Reply style: the admin's own saved style (public profiles only).
  let picking = $state(false);
  let saving = $state(false);
  let saveError = $state('');
  function openPicker() {
    saveError = '';
    if (!personas.data) void personas.load();
    picking = true;
  }
  async function saveStyle(key: string, name: string) {
    const id = me.data?.member?.id;
    if (!id || saving) return;
    saving = true;
    saveError = '';
    const outcome = await send((client) => client.patch<MemberRow>(`/api/admin/members/${encodeURIComponent(id)}`, { persona: key }));
    saving = false;
    if (!outcome.ok) {
      saveError = outcome.message;
      return;
    }
    picking = false;
    await me.refresh();
    const style = me.data?.member?.reply_style;
    const by = style?.role_name ? `Your ${style.role_name} role` : 'A role';
    const role = style?.source === 'role' ? ` ${by} still sets ${style.in_effect?.name ?? 'the style'}.` : '';
    toaster.show({ message: `Saved ${name} as your reply style.${role}`, tone: 'ok' });
  }

  // The allowance's "resets in" runs on the read's server clock advanced by
  // monotonic time (as Reminders), ticking each second while a reset is
  // pending; once due, one quiet re-read brings the new window.
  let receivedAt = $state(performance.now());
  let monotonic = $state(performance.now());
  $effect(() => {
    if (me.data) receivedAt = monotonic = performance.now();
  });
  const resetsAt = $derived(me.data?.member?.allowance?.resets_at ?? null);
  $effect(() => {
    if (!resetsAt) return;
    const id = setInterval(() => (monotonic = performance.now()), 1000);
    return () => clearInterval(id);
  });
  const now = $derived(me.data ? serverNow(me.data.server_time, receivedAt, monotonic) : null);
  let reread = '';
  $effect(() => {
    if (!resetsAt || now === null || now < Date.parse(resetsAt) || reread === resetsAt) return;
    reread = resetsAt;
    // An unchanged answer keeps the shown object, so restart the clock here too.
    void me.refresh().then((failed) => {
      if (!failed) receivedAt = monotonic = performance.now();
    });
  });

  const name = $derived(me.data ? (me.data.member?.name ?? me.data.display) : '');
  const count = $derived(sessions.data?.sessions.length ?? null);
  const style = $derived(me.data?.member?.reply_style ?? null);
</script>

<PageLine class="pageline--echo">
  <h1>Account</h1>
  {#if me.data}<p class="pageline__context">{name} · {methodLong(me.data.method)}</p>{/if}
</PageLine>

<section class="card account-window window-fill" class:account-window--compact={compact} aria-labelledby="account-title" data-fid="window">
  <div class="card__head tabs__strip" data-fid="window-bar">
    <h2 class="vh" id="account-title">Account</h2>
    <div class="tabs__tabs" role="tablist" aria-label="Account" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        <button
          class="tabs__tab"
          role="tab"
          type="button"
          id="account-tab-{t.id}"
          aria-selected={tab === t.id}
          aria-controls="account-panel"
          tabindex={tab === t.id ? 0 : -1}
          bind:this={tabs[t.id]}
          onclick={() => choose(t.id)}
          onkeydown={(event) => tabKey(event, index)}
          >{compact && t.id === 'browser' ? 'Browser' : t.label}{#if t.id === 'sessions' && count !== null}<span class="tabs__count">{count}</span>{/if}</button
        >
      {/each}
    </div>
  </div>
  <div class="account-window__body">
    {#if me.data}<Identity me={me.data} {timeZone} {compact} oncopy={() => void copyDiagnostics()} {onsignout} />{/if}
    <div class="account-window__panel" id="account-panel" role="tabpanel" aria-labelledby="account-tab-{tab}" tabindex="0">
      {#if tab === 'browser'}
        <BrowserTab />
      {:else if me.error && !me.data}
        <LoadError thing="your account" reason={me.error} onretry={() => void me.load()} />
      {:else if !me.data}
        <LoadingState text="Loading your account…" />
      {:else if tab === 'sessions'}
        <SessionsTab {sessions} {timeZone} {compact} {busy} onend={(handle, device) => void endSession(handle, device)} onendothers={() => void endOthers()} />
      {:else}
        <ProfileTab me={me.data} {now} {compact} {checking} onrecheck={() => void recheck()} onstyle={openPicker} oncopy={() => void copyDiagnostics()} {onsignout} />
      {/if}
    </div>
  </div>
  <!-- Inside the window: a direct child of the shell would take `.shell > *`'s zero margin and lose its centring. -->
  {#if style}
    <ReplyPicker bind:open={picking} {style} {personas} {saving} error={saveError} onsave={(key, label) => void saveStyle(key, label)} />
  {/if}
</section>
