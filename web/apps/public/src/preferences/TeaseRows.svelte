<!--
  Account › Profile › How Kanade reaches you: OK to be teased (boards
  M_ProfileTease, …Off, …GlobalOff and their phone boards). SwitchRow's
  markup with the Account row's lead; the switch applies at once with Undo on
  the toast and stays usable while teasing is off for everyone (the chip
  says so), so a member can always opt out or in ahead of time. Under it the
  admins' running-joke note, read-only, with whether it is being sent; no
  note, no line.
-->
<script lang="ts">
  import { Icon, type Toaster } from '@kanade/ui';
  import type { MemberPreferences } from './preferences.svelte';
  import './tease.scss';

  let { preferences, phone, toaster }: { preferences: MemberPreferences; phone: boolean; toaster: Toaster } = $props();

  const prefs = $derived(preferences.data);

  async function set(on: boolean, undo: boolean) {
    const error = await preferences.setTease(on);
    if (error === null) return;
    if (error) {
      toaster.show({ message: `Couldn't save your choice: ${error}`, tone: 'error' });
      return;
    }
    const message = on ? 'You’re OK to be teased.' : 'You’re no longer OK to be teased.';
    if (undo) toaster.show({ message, tone: 'ok', action: { label: 'Undo', run: () => void set(!on, false) } });
    else toaster.show({ message, tone: 'ok' });
  }
</script>

<li class="account-row" class:account-row--tight={phone} data-fid="account-tease">
  <span class="account-lead account-lead--accent"><Icon name="tag" /></span>
  <span class="account-row__text">
    <span class="account-row__title" id="acct-tease-t">OK to be teased</span>
    <span class="account-row__sub" id="acct-tease-d"
      >{#if prefs}{prefs.teasing_enabled
          ? 'Reminder headers may joke about your share of the party’s estimate. Your name is never sent to the chat model.'
          : 'Teasing is off for everyone right now. Your choice is kept for when an admin turns it on.'}{:else if preferences.error}Couldn't load your choice: {preferences.error}{:else}Loading
        your choice…{/if}</span
    >
    {#if prefs && !prefs.teasing_enabled}<span class="status-chip status-chip--warn" data-fid="account-tease-paused">off for everyone</span>{/if}
  </span>
  {#if prefs}
    <button
      type="button"
      role="switch"
      class="switch"
      aria-checked={prefs.tease_ok}
      aria-labelledby="acct-tease-t"
      aria-describedby="acct-tease-d"
      aria-disabled={preferences.busy}
      data-fid="account-tease-switch"
      onclick={() => {
        if (!preferences.busy) void set(!prefs.tease_ok, true);
      }}><span class="switch__knob" aria-hidden="true"></span></button
    >
  {:else if preferences.error}
    <button type="button" class="btn" onclick={() => void preferences.load()}>Try again</button>
  {/if}
</li>
{#if prefs?.running_joke}
  <li class="account-row account-row--sub tease-joke" class:account-row--tight={phone} data-fid="account-tease-note">
    <span class="account-row__text">
      <span class="cap">Running joke · from the admins</span>
      <span class="tease-joke__text">“{prefs.running_joke}”</span>
    </span>
    <span class="tease-joke__state"
      >{!prefs.teasing_enabled ? 'Kept; not sent while teasing is off' : prefs.tease_ok ? 'Sent with your share' : 'Kept; not sent while you’re off'}</span
    >
  </li>
{/if}
