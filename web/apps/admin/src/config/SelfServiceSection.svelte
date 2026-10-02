<!--
  Self-service mode (guild-wide): how the extractor and chatbot answer a
  detected change. Links carry the run id and the proposed time only — no
  secrets; members sign in with Discord. Cards-only is v4's behaviour.
-->
<script lang="ts">
  import type { ConfigView, SelfServiceMode } from '@kanade/api-types';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';
  import SwitchCard from './SwitchCard.svelte';

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
  let saving = $state(false);
  const name = (id: SelfServiceMode) => MODES.find((m) => m.id === id)!.name.toLowerCase();

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    saving = true;
    error = await save({ self_service: { mode } }, `Self-service is ${name(mode)}.`);
    saving = false;
  }
</script>

<SettingsPanel title="Self-service">
  {#snippet lead()}Members fix their own runs through a pre-filled portal link.{/snippet}
  <details class="settings__box settings__about">
    <summary>How self-service works</summary>
    <p>
      When the extractor or chatbot spots a change the author could make themselves — their own run, inside this boss week — it answers with a
      pre-filled link to the public portal ("Move X to Wednesday 21:30?", editable, then confirmed; the server re-validates everything).
      Changes to other people's runs keep the ✅ card for approval, and fixed-run changes link the request form instead.
    </p>
  </details>
  <SwitchCard
    title={() => 'Public portal'}
    on={selfService.public_portal}
    stateOn="open"
    stateOff="closed"
    confirm="Open the public portal?"
    confirmOn
    action={(next) => (next ? 'Open the public portal' : 'Close the public portal')}
    apply={(on) =>
      save({ self_service: { public_portal: on } }, on ? 'The public portal is open.' : 'The public portal is closed.', {
        patch: { self_service: { public_portal: !on } },
        done: on ? 'The public portal is closed again.' : 'The public portal is open again.',
      })}
    >While closed, the public origin serves only the app shell, a status endpoint and the bot identity: no schedule, no art. Only admins can switch
    it.</SwitchCard
  >
  <form class="settings__card" data-fid="cfg-card" id="{uid}-form" onsubmit={submit} aria-describedby="{uid}-effect">
    <fieldset class="settings__choices">
      <legend class="settings__cardtitle">How members are answered</legend>
      {#each MODES as m (m.id)}
        <label class="settings__choice">
          <input type="radio" name="{uid}-mode" value={m.id} bind:group={mode} />
          <strong>{m.name}</strong>
          {#if m.id === selfService.mode}<span class="cap settings__current">current</span>{/if}
          <small>{m.what}</small>
        </label>
      {/each}
    </fieldset>
    {#if !selfService.public_portal && mode !== 'cards_only'}
      <p class="settings__box settings__box--warn">⚠ Links only work while the public portal is open.</p>
    {/if}
    {#if error}<p class="field__error" role="alert">{error}</p>{/if}
    <p class="settings__cardnote" id="{uid}-effect" role="status">
      In effect: <strong>{name(selfService.effective_mode)}</strong>
      {#if selfService.effective_mode !== selfService.mode}
        — the public portal is closed, so cards-only applies. The saved choice ({name(selfService.mode)}) returns when it reopens.
      {/if}
    </p>
  </form>
  {#snippet bar()}
    <SaveBar
      form="{uid}-form"
      label="Save self-service"
      changes={mode === selfService.mode ? [] : [{ label: 'Answer with', from: name(selfService.mode), to: name(mode) }]}
      {saving}
      ondiscard={() => {
        mode = selfService.mode;
        error = '';
      }}
    />
  {/snippet}
</SettingsPanel>

<style>
  .settings__about {
    display: block;
  }

  .settings__about summary {
    font-weight: 600;
    cursor: pointer;
  }

  .settings__about p {
    margin: 0.5rem 0 0;
  }

  .settings__choices legend {
    margin-bottom: 0.5rem;
    padding: 0;
  }
</style>
