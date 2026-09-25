<!--
  Persona catalog plus reply profiles. Profile text lives in files under
  config/personas/profiles/ and is shown here read-only, as v4's table; the
  app reloads them after a file edit. Publishing and role assignments are
  shown but not editable until the server can store them (422 read_only).
-->
<script lang="ts">
  import type { ConfigView, ReplyProfile } from '@kanade/api-types';
  import { PendingLabel, type Toaster } from '@kanade/ui';
  import Name from '../names/Name.svelte';
  import { send } from '../resource.svelte';
  import { plainText } from './markdown';
  import type { Save } from './save';

  let {
    persona,
    save,
    toaster,
    refresh,
  }: { persona: ConfigView['persona']; save: Save; toaster: Toaster; refresh: () => Promise<void> } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let active = $state(persona.active);
  // svelte-ignore state_referenced_locally
  let profiles = $state<ReplyProfile[]>(persona.profiles.map((p) => ({ ...p })));
  let personaError = $state('');
  let reloading = $state(false);
  const profileName = (key: string) => profiles.find((p) => p.key === key)?.name ?? key;
  const current = $derived(persona.personas.find((p) => p.key === persona.active));

  async function usePersona(event: SubmitEvent) {
    event.preventDefault();
    const next = persona.personas.find((p) => p.key === active);
    personaError = await save({ persona: { active } }, `Kanade now speaks as ${next?.name ?? active}.`);
  }

  async function reload() {
    if (reloading) return;
    reloading = true;
    const result = await send((c) => c.post<{ message: string }>('/api/admin/config/profiles/reload', {}));
    // Re-read the config so each profile's voice and summary reflect the files.
    if (result.ok) {
      await refresh();
      profiles = persona.profiles.map((p) => ({ ...p }));
    }
    reloading = false;
    toaster.show({ message: result.ok ? result.value.message : `Couldn't reload: ${result.message}`, tone: result.ok ? 'ok' : 'error' });
  }
</script>

<h3 class="settings__title">Persona</h3>
<form class="filters" onsubmit={usePersona} aria-describedby="{uid}-help">
  <label class="field"
    ><span>Active persona</span>
    <select bind:value={active}>
      {#each persona.personas as p (p.key)}<option value={p.key}>{p.name}</option>{/each}
    </select>
  </label>
  <button class="btn btn--primary" type="submit" disabled={!persona.personas.length}>Use this persona</button>
</form>
<p class="field__error" role="alert">{personaError}</p>
<p class="note" id="{uid}-help">Changing persona swaps identity, default behaviour and staging together.</p>
<p class="note" role="status">Effective: <strong>{current?.name ?? persona.active}</strong> <code>{current?.bundle ?? ''}</code></p>

<h4 class="settings__subtitle">Reply profiles</h4>
<p class="note">
  Profile text is edited as files under <code>config/personas/profiles/</code>; the app shows what is there and reloads it after an
  edit. Publishing offers a profile when members pick their reply style on Members.
</p>
<div class="settings__actions">
  <button class="btn" type="button" aria-disabled={reloading} onclick={() => void reload()}
    ><PendingLabel pending={reloading} label="Reloading…">Reload profiles</PendingLabel></button
  >
</div>
<p class="note" id="{uid}-ro">Publishing a profile and assigning profiles to roles are not editable here yet.</p>
<div class="table-wrap">
  <table class="settings__profiles">
    <caption class="vh">Reply profiles</caption>
    <thead>
      <tr><th scope="col">Profile</th><th scope="col">Voice</th><th scope="col">Prompt</th><th scope="col">Visibility</th><th scope="col"><span class="vh">Actions</span></th></tr>
    </thead>
    <tbody>
      {#each profiles as p (p.key)}
        <tr>
          <th scope="row">{p.name}</th>
          <td>{p.voice}</td>
          <td class="note">{plainText(p.prompt_summary)}</td>
          <td><span class="chip {p.public ? 'chip--yes' : 'chip--waiting'}">{p.public ? 'public' : 'private'}</span></td>
          <td>
            <button class="btn" type="button" disabled aria-describedby="{uid}-ro"
              >{p.public ? 'Make private' : 'Make public'}<span class="vh"> {p.name}</span></button
            >
          </td>
        </tr>
      {:else}
        <tr><td colspan="5" class="note">No reply profiles.</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<h4 class="settings__subtitle">Reply profile per Discord role</h4>
<p class="note">If a member holds several of these roles, the first matching role in this order wins. Assignments never grant chatbot access.</p>
<div class="table-wrap">
  <table aria-describedby="{uid}-ro">
    <caption>A member with one of these roles gets its profile unless they picked their own on Members.</caption>
    <thead><tr><th scope="col" class="num">Order</th><th scope="col">Role</th><th scope="col">Profile</th></tr></thead>
    <tbody>
      {#each persona.role_profiles as role, index (index)}
        <tr>
          <td class="num">{index + 1}</td>
          <th scope="row">{#if role.role_id}<Name kind="role" id={role.role_id} name={role.role_name} />{:else}{role.role_name}{/if}</th>
          <td>{profileName(role.profile)}</td>
        </tr>
      {:else}
        <tr><td colspan="3" class="note">No role assignments; everyone gets the persona's default.</td></tr>
      {/each}
    </tbody>
  </table>
</div>
