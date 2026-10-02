<!--
  A switch card (spec "When changes apply"): a switch applies at once and never
  marks the section dirty. Turning something off (or, with `confirmOn`, on)
  asks first; the toast that follows offers Undo (the caller's `apply`).
-->
<script lang="ts">
  import { Modal } from '@kanade/ui';
  import type { Snippet } from 'svelte';

  let {
    title,
    children,
    on,
    stateOn = 'on',
    stateOff = 'off',
    confirm,
    confirmOn = false,
    action,
    apply,
    disabled = false,
    chip = true,
  }: {
    /** The card's title for each state, e.g. "Chatbot is on". */
    title: (on: boolean) => string;
    children: Snippet;
    on: boolean;
    stateOn?: string;
    stateOff?: string;
    /** The question asked before the guarded direction; omit to never ask. */
    confirm?: string;
    /** Ask before turning on instead of off (opening the public portal). */
    confirmOn?: boolean;
    /** The confirm button's words, e.g. "Turn the chatbot off". */
    action: (next: boolean) => string;
    apply: (next: boolean) => Promise<string>;
    disabled?: boolean;
    /** Show the on/off state chip after the title (the boards' Watching, Quiet mode). */
    chip?: boolean;
  } = $props();
  const uid = $props.id();

  let busy = $state(false);
  let error = $state('');
  let asking = $state(false);

  async function flip(next: boolean) {
    if (busy) return;
    busy = true;
    error = await apply(next);
    busy = false;
  }

  function press() {
    if (busy || disabled) return;
    const next = !on;
    if (confirm && next === confirmOn) asking = true;
    else void flip(next);
  }
</script>

<div class="settings__card settings__switchcard" data-fid="cfg-card">
  <div class="settings__switchtext">
    <p class="settings__cardtitle" id="{uid}-t">
      {title(on)}
      {#if chip}<span class="status-chip status-chip--{on ? 'ok' : 'neutral'} settings__state">{on ? stateOn : stateOff}</span>{/if}
    </p>
    <p class="settings__cardnote" id="{uid}-d">{@render children()}</p>
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </div>
  <button
    type="button"
    role="switch"
    class="switch"
    data-fid="cfg-switch"
    aria-checked={on}
    aria-labelledby="{uid}-t"
    aria-describedby="{uid}-d"
    aria-disabled={busy || disabled}
    onclick={press}><span class="switch__knob" aria-hidden="true"></span></button
  >
</div>

{#if confirm}
  <Modal bind:open={asking} title={confirm} narrow>
    <p>{confirmOn ? 'Members will see this as soon as it is on.' : 'You can undo it from the message that follows.'}</p>
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Cancel</button>
      <button
        class="btn btn--primary"
        type="button"
        onclick={() => {
          close();
          void flip(!on);
        }}>{action(!on)}</button
      >
    {/snippet}
  </Modal>
{/if}
