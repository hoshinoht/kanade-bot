<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { PendingLabel } from '@kanade/ui';
  import type { Save } from './save';
  import Toggle from './Toggle.svelte';

  let { chatbot, save }: { chatbot: ConfigView['chatbot']; save: Save } = $props();

  // svelte-ignore state_referenced_locally
  let member = $state({ ...chatbot.member_rate });
  // svelte-ignore state_referenced_locally
  let guild = $state({ ...chatbot.guild_rate });
  let error = $state('');
  let saving = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    error = await save({ chatbot: { member_rate: member, guild_rate: guild } }, 'Answer limits saved.');
    saving = false;
  }
</script>

<h3 class="settings__title">Chatbot</h3>
<div class="settings__actions">
  <Toggle
    on={chatbot.enabled}
    onLabel="Turn the chatbot off"
    offLabel="Turn the chatbot on"
    disabled={!chatbot.configured}
    apply={(on) => save({ chatbot: { enabled: on } }, on ? 'The chatbot is on.' : 'The chatbot is off.')}
  />
</div>
{#if chatbot.configured}
  <p class="note">The chatbot is <strong>{chatbot.enabled ? 'on' : 'off'}</strong>. Its models are under Models; who it answers is on Members.</p>
{:else}
  <p class="flash flash--error" role="alert">
    Not configured — set {#each chatbot.missing_env as name, i (i)}{i > 0 ? ' and ' : ''}<code>{name}</code>{/each} in the
    environment and restart. Until both are set the bot answers nobody.
  </p>
{/if}
<h4 class="settings__subtitle">How often it answers</h4>
<!-- One line per limit, read as a sentence: "Per person  [4] answers every [300] seconds". -->
<form onsubmit={submit}>
  <div class="rates" role="group" aria-label="Answer limits">
    <span class="rates__who">Per person</span>
    <input type="number" min="1" max="100" bind:value={member.count} aria-label="Answers per person" />
    <span class="rates__unit">answers every</span>
    <input type="number" min="10" max="86400" bind:value={member.window_s} aria-label="Their window (s)" />
    <span class="rates__unit">seconds</span>
    <span class="rates__who">Whole guild</span>
    <input type="number" min="1" max="100" bind:value={guild.count} aria-label="Answers per guild" />
    <span class="rates__unit">answers every</span>
    <input type="number" min="10" max="86400" bind:value={guild.window_s} aria-label="Its window (s)" />
    <span class="rates__unit">seconds</span>
  </div>
  <div class="settings__actions">
    <button class="btn btn--primary" type="submit"><PendingLabel pending={saving} label="Saving…">Save</PendingLabel></button>
  </div>
</form>
<p class="field__error" role="alert">{error}</p>
<p class="note">Per-person overrides and live windows are on <a href="/limits">Limits</a>.</p>

<style>
  /* Label, value, unit in fixed columns so the two limits line up and never wrap raggedly. */
  .rates {
    display: grid;
    grid-template-columns: max-content 5.5rem max-content 6.5rem max-content;
    align-items: center;
    gap: 0.45rem 0.6rem;
  }

  .rates input {
    width: 100%;
    text-align: right;
    font-family: var(--mono);
  }

  .rates__who {
    font-weight: 600;
  }

  .rates__unit {
    color: var(--dim-text);
    font-size: var(--fs-small);
    white-space: nowrap;
  }

  @media (max-width: 480px) {
    /* Narrow: the label takes its own line; value and unit columns stay aligned. */
    .rates {
      grid-template-columns: 4.5rem max-content 5rem max-content;
    }

    .rates__who {
      grid-column: 1 / -1;
    }
  }
</style>

