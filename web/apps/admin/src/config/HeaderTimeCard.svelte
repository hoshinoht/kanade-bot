<!--
  Reminder header rewrites (Notifications): the guild-local time of the daily
  batch that writes the persona headers for the next 24 h. Saves on its own,
  with Undo on the toast; a change applies from the next occurrence.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { Save } from './save';
  import { validClock } from './headerTime';

  let { time, save }: { time: string; save: Save } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let draft = $state(time);
  let seen = untrack(() => time);
  $effect(() => {
    // A save elsewhere (or Undo) replaces what the field shows.
    if (time !== seen) {
      seen = time;
      draft = time;
    }
  });
  let busy = $state(false);
  let error = $state('');
  const changed = $derived(draft.trim() !== time);
  const bad = $derived(changed && !validClock(draft));

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !changed) return;
    if (bad) {
      error = 'The time is HH:MM, for example 03:30.';
      return;
    }
    const before = time;
    const next = draft.trim();
    busy = true;
    error = await save({ notifications: { header_generation_time: next } }, `Reminder headers are written daily at ${next}.`, {
      patch: { notifications: { header_generation_time: before } },
      done: `Back to ${before}.`,
    });
    busy = false;
  }
</script>

<form class="settings__card settings__switchcard" data-fid="cfg-card" aria-labelledby="{uid}-t" onsubmit={submit}>
  <div class="settings__switchtext">
    <p class="settings__cardtitle" id="{uid}-t">Reminder header rewrites</p>
    <p class="settings__cardnote" id="{uid}-d">
      Lines for the next 24 h are written then; runs added or moved later get theirs when first seen.
    </p>
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  </div>
  <div class="headertime">
    <label class="headertime__field"
      >Generate daily at <input
        class="mono"
        class:settings__changed={changed}
        bind:value={draft}
        size="6"
        inputmode="numeric"
        autocomplete="off"
        aria-describedby="{uid}-d"
        aria-invalid={bad}
      /></label
    >
    <button type="submit" class="btn" aria-disabled={busy || !changed}>Save</button>
  </div>
</form>

<style>
  .headertime {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .headertime__field {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }

  .headertime__field input {
    width: 6ch;
  }
</style>
