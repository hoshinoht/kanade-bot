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
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import TextModal from '../shared/TextModal.svelte';
  import { plainLines, plainText, preview } from './markdown';
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

  // Search, visibility and page are this section's own state, so a Reload keeps them.
  let search = $state('');
  let visibility = $state<'all' | 'public' | 'private'>('all');
  let page = $state(1);
  const PER_PAGE = 10;
  const needle = $derived(search.trim().toLowerCase());
  const matching = $derived(
    profiles.filter(
      (p) =>
        (visibility === 'all' || (visibility === 'public') === p.public) &&
        (!needle || [p.name, p.voice, plainText(p.prompt_summary)].some((t) => t.toLowerCase().includes(needle))),
    ),
  );
  $effect(() => {
    void [needle, visibility];
    page = 1;
  });
  const shown = $derived(paged(matching, page, PER_PAGE));
  let promptOf = $state<ReplyProfile | null>(null);
  let promptOpen = $state(false);
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
<div class="filters" role="search" aria-label="Find a reply profile">
  <label class="field field--grow"><span>Search</span><input type="search" bind:value={search} placeholder="name, voice or prompt" /></label>
  <label class="field"
    ><span>Visibility</span>
    <select bind:value={visibility}><option value="all">all</option><option value="public">public</option><option value="private">private</option></select>
  </label>
</div>
<div class="table-wrap">
  <table class="settings__profiles">
    <caption class="vh">Reply profiles</caption>
    <thead>
      <tr><th scope="col">Profile</th><th scope="col">Voice</th><th scope="col">Prompt</th><th scope="col">Visibility</th></tr>
    </thead>
    <tbody>
      {#each shown.rows as p (p.key)}
        <tr>
          <th scope="row">{p.name}</th>
          <td>{p.voice}</td>
          <td class="profile__prompt">
            <button
              type="button"
              class="profile__open"
              onclick={() => {
                promptOf = p;
                promptOpen = true;
              }}>{preview(plainText(p.prompt_summary))}<span class="vh">, read {p.name}'s whole prompt</span></button
            >
          </td>
          <td><span class="tone tone--{p.public ? 'success' : 'neutral'}">{p.public ? 'public' : 'private'}</span></td>
        </tr>
      {:else}
        <tr><td colspan="4" class="note">{profiles.length ? 'No profile matches.' : 'No reply profiles.'}</td></tr>
      {/each}
    </tbody>
  </table>
</div>
<Pager bind:page pages={shown.pages} total={matching.length} size={PER_PAGE} noun="profile" back="← Previous" forward="Next →" />
<TextModal bind:open={promptOpen} title={promptOf ? `${promptOf.name}: prompt` : 'Prompt'} eyebrow="Reply profile" text={promptOf ? plainLines(promptOf.prompt_summary) : ''} {toaster} copied="Prompt copied." />

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

<style>
  .profile__prompt {
    max-width: 24rem;
  }

  /* The preview reads as the text it opens; the dotted underline says it acts. */
  .profile__open {
    display: block;
    max-width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: var(--dim-text);
    font: inherit;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-decoration: underline dotted;
    text-underline-offset: 0.2em;
    cursor: pointer;
  }
</style>
