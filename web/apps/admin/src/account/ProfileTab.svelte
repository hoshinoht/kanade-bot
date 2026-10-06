<!--
  Account › Profile (A2 access with A8 recheck, A3 chat allowance, reply
  style, A6/A7 links). Token and Tailscale sign-ins have no member: one
  neutral note, then only the links that still apply. On phones the
  diagnostics and Sign out join here as "This sign-in".
-->
<script lang="ts">
  import type { Me } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { ACCESS, inEffectName, windowWords } from './account';
  import { resetSpan } from '../limits/view';

  let {
    me,
    now = null,
    compact = false,
    checking = false,
    onrecheck,
    onstyle,
    oncopy,
    onsignout,
  }: {
    me: Me;
    /** The server's clock (epoch ms) for "resets in". */
    now?: number | null;
    compact?: boolean;
    checking?: boolean;
    onrecheck: () => void;
    onstyle: () => void;
    oncopy: () => void;
    onsignout: () => void;
  } = $props();

  const member = $derived(me.member);
  const access = $derived(member ? (ACCESS[member.access] ?? ACCESS.none!) : null);
  const allowance = $derived(member?.allowance ?? null);
  const resets = $derived(resetSpan(allowance?.resets_at, now));
  const style = $derived(member && member.access !== 'none' ? member.reply_style : null);
  const roleWords = $derived(style?.role_name ? `your ${style.role_name} role` : 'your role');
  // The phone row's one line: who sets it, then the saved style.
  const compactStyle = $derived(style ? `${style.source === 'role' ? `set by ${roleWords} · ` : ''}saved: ${style.saved?.name ?? 'Default voice'}` : '');
  // History's actor filter for this sign-in; a Tailscale login is not known here.
  const actor = $derived(member ? `admin:discord:${member.id}` : me.method === 'token' ? 'admin:token' : '');

  /** The meter's used share, through CSSOM (no inline style under the CSP). */
  function used(node: HTMLElement, share: number) {
    const set = (value: number) => node.style.setProperty('--used', `${Math.round(Math.min(1, Math.max(0, value)) * 1000) / 10}%`);
    set(share);
    return { update: set };
  }

  function roleColor(node: HTMLElement, color: string | undefined) {
    const set = (value: string | undefined) => (value ? node.style.setProperty('--role', value) : node.style.removeProperty('--role'));
    set(color);
    return { update: set };
  }
</script>

<div class="account-col">
  {#if !member}
    <p class="settings__box" data-fid="account-note">
      <Icon name="info" />
      <span>
        <b>Not a Discord member.</b>
        {#if me.method === 'discord'}
          Your Discord account has no member row right now, so there is no member access, chat allowance or reply style to show.
        {:else}
          This session signed in with {me.method === 'token' ? 'an access token' : 'Tailscale'}, so there is no member access, chat allowance or portrait to
          show. Sign in with Discord to see your own.
        {/if}
      </span>
    </p>
  {:else}
    <section class="account-sec" aria-labelledby="account-access" data-fid="account-access">
      <div class="account-sec__head">
        <h3 class="cap" id="account-access">Access</h3>
        <span class="account-sec__end account-sec__note" aria-live="polite">{checking ? 'Checking…' : 'Checked just now:'} {#if !checking}<b>{access?.label}</b>{/if}</span>
        <button type="button" class="btn" onclick={onrecheck} aria-disabled={checking}>{compact ? 'Recheck' : 'Recheck my access'}</button>
      </div>
      <ul class="account-grp">
        <li class="account-row">
          <span class="account-lead account-lead--flower account-lead--accent"><Icon name="message-square" /></span>
          <span class="account-row__text"><span class="account-row__title">Chatbot access</span><span class="account-row__sub">{access?.sub}</span></span>
          {#if member.access === 'none'}<span class="status-chip status-chip--neutral">None</span>{:else}<span class="account-chip-acc">{access?.label}</span>{/if}
        </li>
        <li class="account-row">
          <span class="account-lead account-lead--ok"><Icon name="shield" /></span>
          <span class="account-row__text"><span class="account-row__title">Bossing role</span><span class="account-row__sub">{member.bossing ? 'On the roster' : 'Not on the roster'}</span></span>
          {#if member.bossing}<span class="status-chip status-chip--ok"><Icon name="check" />Yes</span>{:else}<span class="status-chip status-chip--neutral">No</span>{/if}
        </li>
        <li class="account-row">
          <span class="account-lead"><Icon name="tag" /></span>
          <span class="account-row__text">
            <span class="account-row__title">Server roles</span>
            <span class="account-row__sub">{member.roles === null ? 'Role directory unavailable right now' : member.roles.length ? 'Highest first' : 'No server roles'}</span>
          </span>
          {#if member.roles === null}
            <span class="account-hidden">Hidden</span>
          {:else if member.roles.length}
            <ul class="account-roles account-row__end" aria-label="Server roles">
              {#each member.roles as role (role.id)}<li class="account-role" use:roleColor={role.color}>{role.name}</li>{/each}
            </ul>
          {/if}
        </li>
      </ul>
    </section>

    {#if allowance}
      <section class="account-sec" aria-labelledby="account-allowance" data-fid="account-allowance">
        <div class="account-sec__head">
          <h3 class="cap" id="account-allowance">Chat allowance</h3>
          <a class="account-sec__end account-style__change" href="/limits">{compact ? 'Limits' : 'Open Limits'}</a>
        </div>
        {#if !allowance.allowance}
          <div class="account-grp">
            <div class="account-row">
              <span class="account-lead account-lead--ok"><Icon name="check" /></span>
              <span class="account-row__text"><span class="account-row__title">Exempt (staff)</span><span class="account-row__sub">Staff answers don't count against a budget</span></span>
            </div>
          </div>
        {:else}
          {@const quota = allowance.allowance}
          <div class="account-grp">
            <div class="account-row account-row--stack">
              <span class="account-allow__line"
                ><span><b>{allowance.used}</b> of <b>{quota.count}</b> answers used</span> {#if resets !== null}<span
                    class="account-allow__resets">{#if resets}resets in <b>{resets}</b>{:else}resets now{/if}</span
                  >{/if}</span
              >
              <div
                class="account-meter"
                class:account-meter--empty={allowance.used === 0}
                role="progressbar"
                aria-label="{allowance.used} of {quota.count} answers used"
                aria-valuemin={0}
                aria-valuemax={quota.count}
                aria-valuenow={allowance.used}
                use:used={quota.count ? allowance.used / quota.count : 0}
              >
                {#if allowance.used > 0}<i class="account-meter__used"></i>{/if}{#if allowance.used < quota.count}<i class="account-meter__rest"></i>{/if}
              </div>
              <span class="account-allow__facts"><span>{windowWords(quota.per_s)} rolling window</span><span aria-hidden="true">·</span><span>{allowance.override ? 'Your own allowance (overridden)' : 'Not overridden'}</span></span>
            </div>
          </div>
        {/if}
      </section>
    {/if}

    {#if style}
      <section class="account-sec" aria-labelledby="account-style" data-fid="account-style">
        <div class="account-sec__head">
          <h3 class="cap" id="account-style">Reply style</h3>
          {#if !compact}<span class="account-sec__end account-sec__note">How Kanade talks to you in chat</span>{/if}
        </div>
        {#if compact}
          <div class="account-grp">
            <button type="button" class="account-row" onclick={onstyle} aria-haspopup="dialog">
              <span class="account-lead account-lead--flower account-lead--accent"><Icon name="message-square" /></span>
              <span class="account-row__text">
                <span class="account-row__title">{inEffectName(style)}</span>
                <span class="account-row__sub">{compactStyle}</span>
              </span>
              <span class="account-style__change">Change</span><Icon name="chevron-right" />
            </button>
          </div>
        {:else}
          <div class="account-grp">
            <div class="account-row">
              <span class="account-lead account-lead--flower account-lead--accent"><Icon name="message-square" /></span>
              <span class="account-row__text">
                <span class="cap account-style__eyebrow">In effect</span>
                <span class="account-row__title account-style__title">
                  {inEffectName(style)}
                  {#if style.source === 'role'}<span class="status-chip status-chip--neutral"><Icon name="lock" />set by {roleWords}</span>{/if}
                </span>
              </span>
            </div>
            <div class="account-row account-row--sub">
              <span class="account-row__text">
                <span>Your saved style: <b>{style.saved?.name ?? 'Default voice'}</b></span>
                <span class="account-row__sub">{style.source === 'role' ? 'Kept for you. It takes over when no role sets a style.' : 'Kanade uses it in chat.'}</span>
              </span>
              <button type="button" class="btn" onclick={onstyle} aria-haspopup="dialog">Change…</button>
            </div>
          </div>
        {/if}
      </section>
    {/if}
  {/if}

  <section class="account-sec" aria-labelledby="account-goto">
    <div class="account-sec__head"><h3 class="cap" id="account-goto">Go to</h3></div>
    <div class="account-grp">
      {#if member}
        <a class="account-row" href="/members?open={encodeURIComponent(member.id)}">
          {#if !compact}<span class="account-lead account-lead--accent"><Icon name="users" /></span>{/if}
          <span class="account-row__text"><span class="account-row__title">{compact ? 'My member profile' : 'Open my member profile'}</span><span class="account-row__sub">Always in, attendance and fixed runs</span></span>
          <Icon name="chevron-right" />
        </a>
      {/if}
      {#if actor}
        <a class="account-row" href="/history?actor={encodeURIComponent(actor)}">
          {#if !compact}<span class="account-lead"><Icon name="history" /></span>{/if}
          <span class="account-row__text"><span class="account-row__title">{compact ? 'My changes' : 'See my changes'}</span><span class="account-row__sub">History, filtered to you</span></span>
          <Icon name="chevron-right" />
        </a>
      {:else}
        <a class="account-row" href="/history">
          {#if !compact}<span class="account-lead"><Icon name="history" /></span>{/if}
          <span class="account-row__text"><span class="account-row__title">History</span><span class="account-row__sub">Every change, newest first</span></span>
          <Icon name="chevron-right" />
        </a>
      {/if}
    </div>
  </section>

  {#if compact}
    <section class="account-sec" aria-labelledby="account-signin">
      <div class="account-sec__head"><h3 class="cap" id="account-signin">This sign-in</h3></div>
      <div class="account-grp">
        <div class="account-row account-row--plain">
          <span class="account-row__text"><span class="account-row__title">Diagnostics</span><span class="account-row__sub account-session__mono">{me.method} · {me.version}</span></span>
          <button type="button" class="btn" onclick={oncopy}>Copy</button>
        </div>
      </div>
      <button type="button" class="btn btn--danger account-full" onclick={onsignout}><Icon name="log-out" />Sign out</button>
    </section>
  {/if}
</div>

<style>
  .account-allow__resets {
    margin-left: auto;
    color: var(--dim-text);
    font-size: var(--fs-small);
  }
</style>
