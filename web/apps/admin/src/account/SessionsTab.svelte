<!--
  Account › Sessions (A11): this identity's live sessions, this one first
  and marked; Sign out ends another one at once, "Sign out everywhere else"
  ends every other one. This session ends only through Sign out. Times are
  guild-local; "last seen" reads against the server's clock.
-->
<script lang="ts">
  import type { AccountSessions } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import { METHOD, dayTime, deviceName, isHandheld, seenWords } from './account';

  let {
    sessions,
    timeZone,
    compact = false,
    busy = '',
    onend,
    onendothers,
  }: {
    sessions: Resource<AccountSessions>;
    timeZone: string;
    compact?: boolean;
    /** The handle being signed out, or `others`. */
    busy?: string;
    onend: (handle: string, device: string) => void;
    onendothers: () => void;
  } = $props();

  const rows = $derived(sessions.data?.sessions ?? []);
  const others = $derived(rows.filter((row) => !row.current).length);
  const now = $derived(sessions.data?.generated_at ?? '');
</script>

<div class="account-col account-sessions" data-fid="account-sessions">
  {#if sessions.error && !sessions.data}
    <LoadError thing="your sessions" reason={sessions.error} onretry={() => void sessions.load()} />
  {:else if !sessions.data}
    <LoadingState text="Loading your sessions…" />
  {:else}
    <div class="account-sec__head">
      {#if compact}<h3 class="cap" id="account-sessions-title">Active sessions</h3>{:else}<h3 class="account-sec__title" id="account-sessions-title">Active sessions</h3>{/if}
      {#if !compact && others > 0}
        <button type="button" class="btn btn--danger account-sec__end" onclick={onendothers} aria-disabled={busy !== ''}>Sign out everywhere else</button>
      {/if}
    </div>
    <ul class="account-grp" aria-labelledby="account-sessions-title">
      {#each rows as row (row.handle)}
        {@const name = deviceName(row)}
        <li class="account-row account-session" class:account-session--current={row.current} data-session={row.handle}>
          {#if !compact}<span class="account-lead"><Icon name={isHandheld(row) ? 'smartphone' : 'monitor'} /></span>{/if}
          <span class="account-row__text">
            <span class="account-row__title account-style__title">{name}{#if row.current && !compact}<span class="account-chip-acc">This one</span>{/if}</span>
            <span class="account-row__sub">
              {METHOD[row.method] ?? row.method}
              · {compact ? '' : 'signed in '}<span class="account-session__mono">{dayTime(row.signed_in_at, timeZone)}</span>
              · {compact ? 'seen' : 'last seen'} {#if row.current}<b>now</b>{:else}<span class="account-session__mono">{seenWords(row.last_seen_at, now)}</span>{/if}
            </span>
          </span>
          {#if row.current}
            {#if compact}<span class="account-chip-acc">This one</span>{:else}<span class="account-sec__note">Use Sign out on the left</span>{/if}
          {:else}
            <button type="button" class="btn" onclick={() => onend(row.handle, name)} aria-disabled={busy !== ''} aria-label="Sign out {name}, signed in {dayTime(row.signed_in_at, timeZone)}">
              {busy === row.handle ? 'Signing out…' : 'Sign out'}
            </button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if compact && others > 0}
      <button type="button" class="btn btn--danger account-full" onclick={onendothers} aria-disabled={busy !== ''}>Sign out everywhere else</button>
    {/if}
    <p class="settings__box">
      <Icon name="info" />
      <span>
        {#if others > 0}
          Signing a session out ends it at once; that device has to sign in again. Everywhere else keeps this one.
        {:else}
          No other sessions. Every session ends after an hour without use, and twelve hours after sign-in at most.
        {/if}
      </span>
    </p>
  {/if}
</div>
