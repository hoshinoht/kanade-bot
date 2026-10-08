<!--
  Signed-in sessions (admin Account › Sessions, A11; the member portal's
  Signed-in devices): this one first and marked; Sign out ends another one at
  once, and `endAll` ends the rest (admin: every other one; member: every one,
  this too). Times are in `timeZone`; "last seen" reads against the server's
  clock (`now`). Each app passes its own rows, words and actions.
-->
<script lang="ts" generics="R extends { handle: string; device: string | null; signed_in_at: string; last_seen_at: string; current: boolean }">
  import type { Snippet } from 'svelte';
  import { dayTime, deviceName, isHandheld, seenWords } from '../account';
  import Icon from './Icon.svelte';
  import LoadError from './LoadError.svelte';
  import LoadingState from './LoadingState.svelte';

  let {
    rows,
    now,
    error = '',
    onretry,
    timeZone,
    compact = false,
    busy = '',
    onend,
    endAll = null,
    title,
    titleId,
    focusableTitle = false,
    thing,
    loading,
    current,
    currentHint = '',
    method,
    note,
  }: {
    /** Null until the first read answers. */
    rows: R[] | null;
    now: string;
    error?: string;
    onretry: () => void;
    timeZone: string;
    compact?: boolean;
    /** The handle being signed out, or the `endAll` action's key. */
    busy?: string;
    onend: (handle: string, device: string) => void;
    /** The end-the-rest action, when it applies. */
    endAll?: { label: string; run: () => void } | null;
    title: string;
    titleId: string;
    /** The heading takes focus after a row's button goes away. */
    focusableTitle?: boolean;
    /** "your sessions": what failed to load. */
    thing: string;
    loading: string;
    /** The mark on this session's row ("This one"). */
    current: string;
    /** Said on this session's row instead of a button (wide only). */
    currentHint?: string;
    /** How a session signed in ("Discord"), leading its line. */
    method?: (row: R) => string;
    /** The note under the list, given how many other sessions there are. */
    note: Snippet<[others: number]>;
  } = $props();
  const others = $derived((rows ?? []).filter((row) => !row.current).length);
</script>

<div class="account-col account-sessions" data-fid="account-sessions">
  {#if error && !rows}
    <LoadError {thing} reason={error} {onretry} />
  {:else if !rows}
    <LoadingState text={loading} />
  {:else}
    <div class="account-sec__head">
      {#if focusableTitle}
        <h3 class={compact ? 'cap' : 'account-sec__title'} id={titleId} tabindex="-1">{title}</h3>
      {:else}
        <h3 class={compact ? 'cap' : 'account-sec__title'} id={titleId}>{title}</h3>
      {/if}
      {#if !compact && endAll}
        <button type="button" class="btn btn--danger account-sec__end" onclick={endAll.run} aria-disabled={busy !== ''}>{endAll.label}</button>
      {/if}
    </div>
    <ul class="account-grp" aria-labelledby={titleId}>
      {#each rows as row (row.handle)}
        {@const name = deviceName(row)}
        <li class="account-row account-session" class:account-session--current={row.current} data-session={row.handle}>
          {#if !compact}<span class="account-lead"><Icon name={isHandheld(row) ? 'smartphone' : 'monitor'} /></span>{/if}
          <span class="account-row__text">
            <span class="account-row__title account-style__title">{name}{#if row.current && !compact}<span class="account-chip-acc">{current}</span>{/if}</span>
            <span class="account-row__sub">
              {#if method}{method(row)} · {/if}{compact ? '' : 'signed in '}<span class="account-session__mono">{dayTime(row.signed_in_at, timeZone)}</span>
              · {compact ? 'seen' : 'last seen'} {#if row.current}<b>now</b>{:else}<span class="account-session__mono">{seenWords(row.last_seen_at, now)}</span>{/if}
            </span>
          </span>
          {#if row.current}
            {#if compact}<span class="account-chip-acc">{current}</span>{:else if currentHint}<span class="account-sec__note">{currentHint}</span>{/if}
          {:else}
            <button type="button" class="btn" onclick={() => onend(row.handle, name)} aria-disabled={busy !== ''} aria-label="Sign out {name}, signed in {dayTime(row.signed_in_at, timeZone)}">
              {busy === row.handle ? 'Signing out…' : 'Sign out'}
            </button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if compact && endAll}
      <button type="button" class="btn btn--danger account-full" onclick={endAll.run} aria-disabled={busy !== ''}>{endAll.label}</button>
    {/if}
    {@render note(others)}
  {/if}
</div>
