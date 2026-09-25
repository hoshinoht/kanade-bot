<!--
  Self-service mode (guild-wide): how the extractor and chatbot answer a
  detected change. Links carry the run id and the proposed time only — no
  secrets; members sign in with Discord. Cards-only is v4's behaviour.
-->
<script lang="ts">
  import type { ConfigView, SelfServiceMode } from '@kanade/api-types';
  import type { Save } from './save';
  import Toggle from './Toggle.svelte';

  let { selfService, save }: { selfService: ConfigView['self_service']; save: Save } = $props();
  const uid = $props.id();

  const MODES: { id: SelfServiceMode; name: string; what: string }[] = [
    { id: 'cards_and_link', name: 'Cards and link', what: 'Keep the ✅ card and add a pre-filled deep link for self-serviceable moves.' },
    { id: 'link_first', name: 'Link first', what: 'Self-serviceable changes get only the pre-filled link.' },
    { id: 'cards_only', name: 'Cards only', what: "v4 behaviour: members answer on the cards, and nothing links out." },
  ];
  // svelte-ignore state_referenced_locally
  let mode = $state<SelfServiceMode>(selfService.mode);
  let error = $state('');
  const name = (id: SelfServiceMode) => MODES.find((m) => m.id === id)!.name.toLowerCase();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    error = await save({ self_service: { mode } }, `Self-service is ${name(mode)}.`);
  }
</script>

<h3 class="settings__title">Self-service</h3>
<p class="note">
  When the extractor or chatbot spots a change the author could make themselves — their own run, inside this boss week — it answers with a
  pre-filled link to the public portal ("Move X to Wednesday 21:30?", editable, then confirmed; the server re-validates everything).
  Changes to other people's runs keep the ✅ card for approval, and fixed-run changes link the request form instead.
</p>
<h4 class="settings__subtitle">Public portal</h4>
<div class="settings__actions">
  <Toggle
    on={selfService.public_portal}
    onLabel="Close the public portal"
    offLabel="Open the public portal"
    apply={(on) => save({ self_service: { public_portal: on } }, on ? 'The public portal is open.' : 'The public portal is closed.')}
  />
</div>
<p class="note">
  The public portal is <strong>{selfService.public_portal ? 'open' : 'closed'}</strong>. Only admins can switch it. While it is closed the
  public origin serves only the app shell, a status endpoint and the bot identity — no schedule, no art — so members answer on cards.
</p>

<h4 class="settings__subtitle">How members are answered</h4>
<form onsubmit={submit} aria-describedby="{uid}-effect">
  <fieldset class="settings__choices">
    <legend class="vh">Self-service mode</legend>
    {#each MODES as m (m.id)}
      <label class="settings__choice">
        <input type="radio" name="{uid}-mode" value={m.id} bind:group={mode} />
        <strong>{m.name}</strong>
        <small>{m.what}</small>
      </label>
    {/each}
  </fieldset>
  <button class="btn btn--primary" type="submit">Save</button>
</form>
<p class="field__error" role="alert">{error}</p>
<p class="note" id="{uid}-effect" role="status">
  In effect: <strong>{name(selfService.effective_mode)}</strong>
  {#if selfService.effective_mode !== selfService.mode}
    — the public portal is closed, so cards-only applies. The saved choice ({name(selfService.mode)}) returns when it reopens.
  {/if}
</p>
