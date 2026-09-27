<!-- Persona catalog plus file-backed reply profiles; visibility is saved separately. -->
<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import type { ConfigView, ReplyProfile, Role } from '@kanade/api-types';
  import { Modal, PendingLabel, type Toaster } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import TextModal from '../shared/TextModal.svelte';
  import { plainLines, plainText, preview } from './markdown';
  import type { Save, SaveRoleProfiles } from './save';
  import RoleAssignmentsSection from './RoleAssignmentsSection.svelte';

  let {
    persona,
    save,
    toaster,
    refresh,
    roles,
    rolesLoading,
    rolesError,
    refreshRoles,
    saveRoleProfiles,
  }: {
    persona: ConfigView['persona'];
    save: Save;
    toaster: Toaster;
    refresh: () => Promise<ConfigView | null>;
    roles: Role[] | null;
    rolesLoading: boolean;
    rolesError: string;
    refreshRoles: () => Promise<void>;
    saveRoleProfiles: SaveRoleProfiles;
  } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  let active = $state(persona.active);
  const profiles = $derived(persona.profiles);
  let personaError = $state('');
  let reloading = $state(false);
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
  const visibleKeys = $derived(shown.rows.map((p) => p.key));
  const selected = new SvelteSet<string>();
  const selectedProfiles = $derived(profiles.filter((p) => selected.has(p.key)));
  const selectedCount = $derived(selected.size);
  const pageSelected = $derived(visibleKeys.length > 0 && visibleKeys.every((key) => selected.has(key)));
  const pagePartlySelected = $derived(visibleKeys.some((key) => selected.has(key)));
  const canPublish = $derived(selectedProfiles.some((p) => !p.public));
  const canMakePrivate = $derived(selectedProfiles.some((p) => p.public));
  let pageCheckbox: HTMLInputElement | undefined = $state();
  let visibilityOpen = $state(false);
  let visibilityTarget = $state<boolean | null>(null);
  let pendingProfiles = $state<ReplyProfile[]>([]);
  let unchangedCount = $state(0);
  let visibilitySaving = $state(false);
  let visibilityError = $state('');
  let visibilityStatus = $state('');
  let promptOf = $state<ReplyProfile | null>(null);
  let promptOpen = $state(false);
  const current = $derived(persona.personas.find((p) => p.key === persona.active));

  $effect(() => {
    if (pageCheckbox) pageCheckbox.indeterminate = pagePartlySelected && !pageSelected;
  });
  $effect(() => {
    const keys = new Set(profiles.map((p) => p.key));
    for (const key of selected) if (!keys.has(key)) selected.delete(key);
  });

  function selectPage(checked: boolean) {
    for (const key of visibleKeys) {
      if (checked) selected.add(key);
      else selected.delete(key);
    }
    visibilityStatus = '';
  }

  function selectProfile(key: string, checked: boolean) {
    if (checked) selected.add(key);
    else selected.delete(key);
    visibilityStatus = '';
  }

  function prepareVisibility(keys: string[], publicValue: boolean) {
    if (visibilitySaving) return;
    const requested = new Set(keys);
    pendingProfiles = profiles.filter((p) => requested.has(p.key) && p.public !== publicValue);
    const unchanged = requested.size - pendingProfiles.length;
    unchangedCount = unchanged;
    if (!pendingProfiles.length) {
      visibilityStatus = requested.size === 1
        ? `That profile is already ${publicValue ? 'public' : 'private'}.`
        : `All ${requested.size} selected profiles are already ${publicValue ? 'public' : 'private'}.`;
      return;
    }
    visibilityTarget = publicValue;
    visibilityError = '';
    visibilityStatus = unchanged
      ? `${unchanged} selected profile${unchanged === 1 ? ' was' : 's were'} already ${publicValue ? 'public' : 'private'}.`
      : '';
    visibilityOpen = true;
  }

  function closeVisibility() {
    if (visibilitySaving) return;
    visibilityOpen = false;
    visibilityError = '';
  }

  async function saveVisibility() {
    if (visibilitySaving || visibilityTarget === null || pendingProfiles.length === 0) return;
    const target = visibilityTarget;
    const changed = [...pendingProfiles];
    const count = changed.length;
    visibilitySaving = true;
    visibilityError = '';
    visibilityError = await save(
      { persona: { visibility: changed.map((p) => ({ key: p.key, public: target })) } },
      target
        ? `Published ${count} reply profile${count === 1 ? '' : 's'}.`
        : `Made ${count} reply profile${count === 1 ? '' : 's'} private.`,
    );
    visibilitySaving = false;
    if (visibilityError) return;
    for (const p of changed) selected.delete(p.key);
    visibilityOpen = false;
    pendingProfiles = [];
    visibilityStatus = target
      ? `Published ${count} reply profile${count === 1 ? '' : 's'}.`
      : `Made ${count} reply profile${count === 1 ? '' : 's'} private.`;
  }

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
{#if personaError}<p class="field__error" role="alert">{personaError}</p>{/if}
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
      <tr>
        <th scope="col">
          <div class="profile__head">
            <label class="profile__select-hit">
              <input
                type="checkbox"
                bind:this={pageCheckbox}
                checked={pageSelected}
                aria-label="Select all reply profiles on this page"
                onchange={(event) => selectPage(event.currentTarget.checked)}
              />
            </label>
            <span>Profile</span>
          </div>
        </th>
        <th scope="col">Voice</th><th scope="col">Prompt</th><th scope="col">Visibility</th>
      </tr>
    </thead>
    <tbody>
      {#each shown.rows as p (p.key)}
        <tr>
          <th scope="row" aria-label={`${p.name}, ${p.public ? 'public' : 'private'}`}>
            <div class="profile__identity">
              <label class="profile__select-hit">
                <input type="checkbox" aria-label="Select {p.name}" checked={selected.has(p.key)} onchange={(event) => selectProfile(p.key, event.currentTarget.checked)} />
              </label>
              <span class="profile__name">{p.name}</span>
              <span class="tone tone--{p.public ? 'success' : 'neutral'} profile__state-narrow">{p.public ? 'public' : 'private'}</span>
              <button class="btn profile__visibility" type="button" aria-disabled={visibilitySaving} onclick={() => prepareVisibility([p.key], !p.public)}>
                {p.public ? 'Make private' : 'Publish'}
              </button>
            </div>
          </th>
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
          <td><span class="tone tone--{p.public ? 'success' : 'neutral'} profile__state-wide">{p.public ? 'public' : 'private'}</span></td>
        </tr>
      {:else}
        <tr><td colspan="4" class="note">{profiles.length ? 'No profile matches.' : 'No reply profiles.'}</td></tr>
      {/each}
    </tbody>
  </table>
</div>
<div class="settings__actions profile__batch" aria-label="Selected reply profiles">
  <span class="note" role="status" aria-live="polite">{selectedCount} profile{selectedCount === 1 ? '' : 's'} selected.{visibilityStatus ? ` ${visibilityStatus}` : ''}</span>
  <button class="btn btn--primary" type="button" disabled={!canPublish || visibilitySaving} onclick={() => prepareVisibility(selectedProfiles.map((p) => p.key), true)}>Publish selected</button>
  <button class="btn" type="button" disabled={!canMakePrivate || visibilitySaving} onclick={() => prepareVisibility(selectedProfiles.map((p) => p.key), false)}>Make private selected</button>
  {#if selectedCount}<button class="btn btn--ghost" type="button" onclick={() => selectPage(false)}>Clear page</button><button class="btn btn--ghost" type="button" onclick={() => { selected.clear(); visibilityStatus = ''; }}>Clear selection</button>{/if}
</div>
{#if visibilityError && !visibilityOpen}<p class="field__error" role="alert">{visibilityError}</p>{/if}
<Pager bind:page pages={shown.pages} total={matching.length} size={PER_PAGE} noun="profile" back="← Previous" forward="Next →" />
<TextModal bind:open={promptOpen} title={promptOf ? `${promptOf.name}: prompt` : 'Prompt'} eyebrow="Reply profile" text={promptOf ? plainLines(promptOf.prompt_summary) : ''} {toaster} copied="Prompt copied." />

<Modal
  bind:open={visibilityOpen}
  dismissible={!visibilitySaving}
  title={visibilityTarget ? `Publish ${pendingProfiles.length} profile${pendingProfiles.length === 1 ? '' : 's'}?` : `Make ${pendingProfiles.length} profile${pendingProfiles.length === 1 ? '' : 's'} private?`}
  eyebrow="Reply profile visibility"
  narrow
>
  <p>
    {visibilityTarget
      ? 'Members will be able to choose these reply styles on Members.'
      : 'Members will no longer be able to choose these reply styles on Members.'}
  </p>
  {#if unchangedCount > 0}<p class="note">{unchangedCount} selected profile{unchangedCount === 1 ? ' is' : 's are'} already {visibilityTarget ? 'public' : 'private'} and will stay unchanged.</p>{/if}
  <ul class="profile__confirm-list">{#each pendingProfiles as p (p.key)}<li>{p.name}</li>{/each}</ul>
  {#if visibilitySaving}<p class="note" role="status" aria-live="polite">{visibilityTarget ? 'Publishing these reply profiles…' : 'Making these reply profiles private…'}</p>{/if}
  {#if visibilityError}<p class="field__error" role="alert">{visibilityError}</p>{/if}
  {#snippet footer(close)}
    <button class="btn" type="button" disabled={visibilitySaving} onclick={() => { close(); closeVisibility(); }}>Cancel</button>
    <button class="btn btn--primary" type="button" aria-disabled={visibilitySaving} onclick={() => void saveVisibility()}>
      <PendingLabel pending={visibilitySaving} label={visibilityTarget ? 'Publishing…' : 'Making private…'}>
        {visibilityTarget ? `Publish ${pendingProfiles.length} profile${pendingProfiles.length === 1 ? '' : 's'}` : `Make ${pendingProfiles.length} profile${pendingProfiles.length === 1 ? '' : 's'} private`}
      </PendingLabel>
    </button>
  {/snippet}
</Modal>

<RoleAssignmentsSection
  profiles={persona.profiles}
  assignments={persona.role_profiles}
  digest={persona.role_profiles_digest}
  {roles}
  {rolesLoading}
  {rolesError}
  {refresh}
  {refreshRoles}
  {saveRoleProfiles}
/>

<style>
  .profile__prompt {
    max-width: 24rem;
  }

  .profile__identity {
    display: grid;
    grid-template-columns: 2.75rem minmax(0, 1fr);
    min-width: 9.5rem;
    align-items: center;
    gap: 0.3rem 0.45rem;
  }

  .settings__profiles tbody th {
    min-width: 11rem;
  }

  .profile__head {
    display: grid;
    grid-template-columns: 2.75rem minmax(0, 1fr);
    align-items: center;
    gap: 0.3rem 0.45rem;
  }

  .profile__head .profile__select-hit {
    grid-column: 1;
    grid-row: 1;
  }

  .profile__head > span {
    grid-column: 2;
  }

  .profile__identity > .profile__select-hit {
    grid-column: 1;
    grid-row: 1 / 4;
  }

  .profile__name,
  .profile__state-narrow,
  .profile__visibility {
    grid-column: 2;
  }

  .profile__visibility {
    min-width: 6rem;
    min-height: 2.5rem;
  }

  .profile__select-hit {
    display: grid;
    min-width: 2.75rem;
    min-height: 2.75rem;
    place-items: center;
    cursor: pointer;
  }

  .profile__select-hit input {
    width: 1.2rem;
    height: 1.2rem;
    padding: 0;
    accent-color: var(--accent);
  }

  .profile__batch {
    align-items: center;
  }

  .profile__batch .note {
    flex: 1 1 100%;
    margin: 0;
  }

  .profile__confirm-list {
    margin: 0.4rem 0 0;
    padding-left: 1.2rem;
  }

  @media (min-width: 900px) {
    .profile__state-narrow {
      display: none;
    }
  }

  @media (max-width: 899px) {
    .settings__profiles th:nth-child(4),
    .settings__profiles td:nth-child(4),
    .profile__state-wide {
      display: none;
    }
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
