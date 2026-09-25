<!--
  Persona catalog plus reply profiles. Profile text lives in files under
  config/personas/profiles/ and is shown here read-only; the app only
  publishes profiles for member choice, assigns them to roles in an order
  (the first matching role wins), and reloads them after a file edit.
-->
<script lang="ts">
  import type { ConfigView, ReplyProfile, RoleProfile } from '@kanade/api-types';
  import { PendingLabel, type Toaster } from '@kanade/ui';
  import { send } from '../resource.svelte';
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
  let roles = $state<RoleProfile[]>(persona.role_profiles.map((r) => ({ ...r })));
  // svelte-ignore state_referenced_locally
  let profiles = $state<ReplyProfile[]>(persona.profiles.map((p) => ({ ...p })));
  let personaError = $state('');
  let rolesError = $state('');
  let reloading = $state(false);
  const current = $derived(persona.personas.find((p) => p.key === persona.active));

  async function usePersona(event: SubmitEvent) {
    event.preventDefault();
    const next = persona.personas.find((p) => p.key === active);
    personaError = await save({ persona: { active } }, `Kanade now speaks as ${next?.name ?? active}.`);
  }

  async function publish(profile: ReplyProfile) {
    const result = await send((c) =>
      c.patch<ConfigView>('/api/admin/config', { persona: { visibility: [{ key: profile.key, public: !profile.public }] } }),
    );
    toaster.show({
      message: result.ok
        ? `${profile.name} is ${profile.public ? 'private; members can no longer choose it' : 'published for member choice'}.`
        : `Couldn't change ${profile.name}: ${result.message}`,
      tone: result.ok ? 'ok' : 'error',
    });
    if (result.ok) profiles = result.value.persona.profiles.map((p) => ({ ...p }));
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

  function move(index: number, direction: -1 | 1) {
    const to = index + direction;
    if (to < 0 || to >= roles.length) return;
    const [item] = roles.splice(index, 1);
    roles.splice(to, 0, item!);
    rolesError = '';
    document.getElementById(`${uid}-role-${to}`)?.focus();
  }

  function remove(index: number) {
    roles.splice(index, 1);
    document.getElementById(`${uid}-add`)?.focus();
    rolesError = '';
  }

  async function saveRoles(event: SubmitEvent) {
    event.preventDefault();
    rolesError = await save({ persona: { role_profiles: roles } }, 'Role profiles saved in this order.');
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
<ul class="settings__profiles">
  {#each profiles as p (p.key)}
    <li>
      <strong>{p.name}</strong>
      <span class="chip {p.public ? 'chip--yes' : 'chip--waiting'}">{p.public ? 'public' : 'private'}</span>
      <br /><span class="note">{p.voice}.</span>
      <br /><span class="note">{p.prompt_summary}</span>
      <div class="settings__actions">
        <button class="btn" type="button" onclick={() => void publish(p)}>{p.public ? 'Make private' : 'Publish for member choice'}</button>
      </div>
    </li>
  {/each}
</ul>

<h4 class="settings__subtitle">Reply profile per Discord role</h4>
<p class="note">If a member holds several of these roles, the first matching role in this order wins. Assignments never grant chatbot access.</p>
<form onsubmit={saveRoles}>
  <div class="table-wrap">
    <table>
      <caption>A member with one of these roles gets its profile unless they picked their own on Members.</caption>
      <thead><tr><th scope="col">Order</th><th scope="col">Role</th><th scope="col">Role id</th><th scope="col">Profile</th><th scope="col"><span class="vh">Remove</span></th></tr></thead>
      <tbody>
        {#each roles as role, index (index)}
          <tr>
            <td>
              <div class="rowbtns">
                <button class="btn" type="button" id="{uid}-role-{index}" disabled={index === 0} onclick={() => move(index, -1)}>↑<span class="vh">Move {role.role_name || `row ${index + 1}`} up</span></button>
                <button class="btn" type="button" disabled={index === roles.length - 1} onclick={() => move(index, 1)}>↓<span class="vh">Move {role.role_name || `row ${index + 1}`} down</span></button>
              </div>
            </td>
            <td><input aria-label="Role name, row {index + 1}" bind:value={role.role_name} autocomplete="off" /></td>
            <td><input class="mono" aria-label="Role id, row {index + 1}" bind:value={role.role_id} inputmode="numeric" autocomplete="off" /></td>
            <td>
              <select aria-label="Profile for {role.role_name || `row ${index + 1}`}" bind:value={role.profile}>
                {#each profiles as p (p.key)}<option value={p.key}>{p.name}{p.public ? '' : ' (private)'}</option>{/each}
              </select>
            </td>
            <td><button class="btn" type="button" onclick={() => remove(index)}>Remove<span class="vh"> {role.role_name || `row ${index + 1}`}</span></button></td>
          </tr>
        {:else}
          <tr><td colspan="5" class="note">No role assignments; everyone gets the persona's default.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="settings__actions">
    <button class="btn" type="button" id="{uid}-add" onclick={() => roles.push({ role_id: '', role_name: '', profile: profiles[0]?.key ?? 'default' })}>Add a role</button>
    <button class="btn btn--primary" type="submit">Save role profiles</button>
  </div>
</form>
<p class="field__error" role="alert">{rolesError}</p>
