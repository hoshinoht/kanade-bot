<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import type { Save } from './save';
  import Toggle from './Toggle.svelte';

  let { chatbot, save }: { chatbot: ConfigView['chatbot']; save: Save } = $props();

  // svelte-ignore state_referenced_locally
  let member = $state({ ...chatbot.member_rate });
  // svelte-ignore state_referenced_locally
  let guild = $state({ ...chatbot.guild_rate });
  let error = $state('');

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    error = await save({ chatbot: { member_rate: member, guild_rate: guild } }, 'Answer limits saved.');
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
<form class="filters" onsubmit={submit}>
  <label class="field"><span>Answers per person</span><input type="number" min="1" max="100" bind:value={member.count} /></label>
  <label class="field"><span>Their window (s)</span><input type="number" min="10" max="86400" bind:value={member.window_s} /></label>
  <label class="field"><span>Answers per guild</span><input type="number" min="1" max="100" bind:value={guild.count} /></label>
  <label class="field"><span>Its window (s)</span><input type="number" min="10" max="86400" bind:value={guild.window_s} /></label>
  <button class="btn btn--primary" type="submit">Save</button>
</form>
<p class="field__error" role="alert">{error}</p>
<p class="note">Per-person overrides and live windows are on <a href="/limits">Limits</a>.</p>
