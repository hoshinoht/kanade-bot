<!-- Config → Persona (B_CfgPersona, B_CfgRoles): the active persona and the file-backed reply profiles on one pill tab, role overrides on the other. -->
<script lang="ts">
  import { SvelteSet } from 'svelte/reactivity';
  import type { ConfigView, ReplyProfile, Role } from '@kanade/api-types';
  import { Modal, PendingLabel, type Toaster } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import Pager from '../pages/Pager.svelte';
  import { paged } from '../pages/paging';
  import TextModal from '../shared/TextModal.svelte';
  import type { Change } from './dirty';
  import { plainLines, plainText, preview } from './markdown';
  import PillTabs from './PillTabs.svelte';
  import type { Save, SaveRoleProfiles } from './save';
  import RoleAssignmentsSection from './RoleAssignmentsSection.svelte';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

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

  let tab = $state('active');
  let roleEditor: { discard: () => void } | undefined = $state();
  let roleChanges = $state<Change[]>([]);
  let roleSaving = $state(false);
  let roleBlocked = $state(false);

  // svelte-ignore state_referenced_locally
  let active = $state(persona.active);
  let personaOpen = $state(false);
  const next = $derived(persona.personas.find((p) => p.key === active));
  const profiles = $derived(persona.profiles);
  let personaError = $state('');
  let reloading = $state(false);
  // Search, visibility and page are this section's own state, so a Reload keeps them.
  let search = $state('');
  let visibility = $state<'all' | 'public' | 'private'>('all');
  const VISIBILITY = [
    { key: 'all', label: 'All' },
    { key: 'public', label: 'Public' },
    { key: 'private', label: 'Private' },
  ] as const;
  const counts = $derived({
    all: profiles.length,
    public: profiles.filter((p) => p.public).length,
    private: profiles.filter((p) => !p.public).length,
  });
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
  // No profile is the default: with no role or member choice the bot speaks in
  // the active persona's own voice. Shown as a fixed first row, never selected,
  // edited or sent; it follows the filters like a public profile would.
  const DEFAULT_LABEL = 'Default voice';
  const DEFAULT_PROMPT = "(the persona's own voice)";
  const defaultVoice = $derived(`${current?.name ?? persona.active} as written`);
  const showDefault = $derived(
    page === 1 && visibility !== 'private' && (!needle || [DEFAULT_LABEL, defaultVoice, DEFAULT_PROMPT].some((t) => t.toLowerCase().includes(needle))),
  );

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
    const noun = `${count} reply profile${count === 1 ? '' : 's'}`;
    visibilityError = await save(
      { persona: { visibility: changed.map((p) => ({ key: p.key, public: target })) } },
      target ? `Published ${noun}.` : `Made ${noun} private.`,
      // Undo restores each profile's own earlier visibility.
      { patch: { persona: { visibility: changed.map((p) => ({ key: p.key, public: p.public })) } }, done: `Restored ${noun}.` },
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

  // An action, not a draft: it applies at once after the confirm (spec "Actions").
  function askPersona(event: SubmitEvent) {
    event.preventDefault();
    personaOpen = true;
  }

  async function usePersona() {
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

<SettingsPanel title="Persona">
  {#snippet tabs()}
    <PillTabs
      id={uid}
      label="Persona"
      bind:selected={tab}
      tabs={[
        { key: 'active', label: 'Active persona & profiles' },
        { key: 'roles', label: 'Role overrides', count: persona.role_profiles.length },
      ]}
    />
  {/snippet}
  <div class="settings__tabpanel" role="tabpanel" id="{uid}-panel-active" aria-labelledby="{uid}-tab-active" hidden={tab !== 'active'}>
    <form class="settings__card settings__card--row persona__active" data-fid="cfg-card" onsubmit={askPersona} aria-describedby="{uid}-help">
      <label class="field"
        ><span>Active persona</span>
        <select bind:value={active} class:settings__changed={active !== persona.active}>
          {#each persona.personas as p (p.key)}<option value={p.key}>{p.name}</option>{/each}
        </select>
      </label>
      <button class="btn btn--primary settings__key" data-fid="cfg-persona-use" type="submit" disabled={!persona.personas.length}>Use this persona</button>
      <p class="settings__cardnote persona__effective" id="{uid}-help">
        Swaps identity, default behaviour and staging together.<br /><span role="status"
          >Effective: <strong>{current?.name ?? persona.active}</strong> <code>{current?.bundle ?? ''}</code></span
        >
      </p>
      {#if personaError}<p class="field__error" role="alert">{personaError}</p>{/if}
    </form>

    <section class="settings__card profiles persona__profiles" data-fid="cfg-card" aria-labelledby="{uid}-profiles">
      <div class="settings__cardhead">
        <h4 class="settings__cardtitle" id="{uid}-profiles">Reply profiles</h4>
        <span class="settings__cardnote">edited as files under <code>config/personas/profiles/</code></span>
        <button class="btn settings__cardaction" type="button" aria-disabled={reloading} onclick={() => void reload()}
          ><PendingLabel pending={reloading} label="Reloading…">Reload profiles</PendingLabel></button
        >
      </div>
      <div class="profiles__tools" data-fid="cfg-profiles-tools">
        <input
          class="profiles__search"
          type="search"
          bind:value={search}
          placeholder="name, voice or prompt"
          aria-label="Search"
          aria-describedby="{uid}-search-help"
        />
        <span class="vh" id="{uid}-search-help">Find a reply profile by name, voice or prompt.</span>
        <div class="seg" role="group" aria-label="Visibility">
          {#each VISIBILITY as v (v.key)}
            <button type="button" aria-pressed={visibility === v.key} onclick={() => (visibility = v.key)}
              >{v.label} <span class="profiles__count">{counts[v.key]}</span></button
            >
          {/each}
        </div>
        <span class="profiles__selected" role="status" aria-live="polite"
          >{selectedCount} profile{selectedCount === 1 ? '' : 's'} selected.{visibilityStatus ? ` ${visibilityStatus}` : ''}</span
        >
        <button class="btn" type="button" aria-label="Publish selected" disabled={!canPublish || visibilitySaving} onclick={() => prepareVisibility(selectedProfiles.map((p) => p.key), true)}
          >Publish</button
        >
        <button
          class="btn"
          type="button"
          aria-label="Make private selected"
          disabled={!canMakePrivate || visibilitySaving}
          onclick={() => prepareVisibility(selectedProfiles.map((p) => p.key), false)}>Make private</button
        >
        {#if selectedCount}<button class="btn btn--ghost" type="button" onclick={() => { selected.clear(); visibilityStatus = ''; }}>Clear</button>{/if}
      </div>
      <div class="profiles__wrap">
        <table class="settings__table profiles__table" data-fid="cfg-profiles">
          <caption class="vh">Reply profiles</caption>
          <thead>
            <tr>
              <th scope="col" class="profiles__pick">
                <input
                  type="checkbox"
                  bind:this={pageCheckbox}
                  checked={pageSelected}
                  aria-label="Select all reply profiles on this page"
                  onchange={(event) => selectPage(event.currentTarget.checked)}
                />
              </th>
              <th scope="col" class="profiles__name">Profile</th>
              <th scope="col" class="profiles__voice">Voice</th>
              <th scope="col">Prompt</th>
              <th scope="col" class="profiles__vis">Visibility</th>
              <th scope="col" class="profiles__act"><span class="vh">Change visibility</span></th>
            </tr>
          </thead>
          <tbody>
            {#if showDefault}
              <tr class="profiles__default">
                <td class="profiles__pick"></td>
                <th scope="row" class="profiles__name" aria-label="{DEFAULT_LABEL}, public, the default">{DEFAULT_LABEL}</th>
                <td class="profiles__voice">{defaultVoice}</td>
                <td class="profile__prompt profile__prompt--default">{DEFAULT_PROMPT}</td>
                <td class="profiles__vis"><span class="status-chip status-chip--ok profile__state-wide">public</span></td>
                <td class="profiles__act"><span class="cap">default</span></td>
              </tr>
            {/if}
            {#each shown.rows as p (p.key)}
              <tr>
                <td class="profiles__pick">
                  <input type="checkbox" aria-label="Select {p.name}" checked={selected.has(p.key)} onchange={(event) => selectProfile(p.key, event.currentTarget.checked)} />
                </td>
                <th scope="row" class="profiles__name" aria-label={`${p.name}, ${p.public ? 'public' : 'private'}`}>{p.name}</th>
                <td class="profiles__voice">{p.voice}</td>
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
                <td class="profiles__vis"><span class="status-chip status-chip--{p.public ? 'ok' : 'neutral'} profile__state-wide">{p.public ? 'public' : 'private'}</span></td>
                <td class="profiles__act">
                  <button class="btn profile__visibility" type="button" aria-disabled={visibilitySaving} onclick={() => prepareVisibility([p.key], !p.public)}>
                    {p.public ? 'Make private' : 'Publish'}
                  </button>
                </td>
              </tr>
            {:else}
              {#if !showDefault || !profiles.length}<tr><td colspan="6" class="note">{profiles.length ? 'No profile matches.' : 'No reply profiles.'}</td></tr>{/if}
            {/each}
          </tbody>
        </table>
      </div>
      {#if visibilityError && !visibilityOpen}<p class="field__error" role="alert">{visibilityError}</p>{/if}
      <Pager bind:page pages={shown.pages} total={matching.length} size={PER_PAGE} noun="profile" back="← Previous" forward="Next →" />
    </section>
  </div>
  <div class="settings__tabpanel" role="tabpanel" id="{uid}-panel-roles" aria-labelledby="{uid}-tab-roles" hidden={tab !== 'roles'}>
    <RoleAssignmentsSection
      bind:this={roleEditor}
      bind:changes={roleChanges}
      bind:saving={roleSaving}
      bind:blocked={roleBlocked}
      form="{uid}-roles"
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
  </div>
  {#snippet bar()}
    {#if tab === 'roles'}
      <SaveBar form="{uid}-roles" label="Save role overrides" changes={roleChanges} saving={roleSaving} disabled={roleBlocked} ondiscard={() => roleEditor?.discard()} />
    {/if}
  {/snippet}
</SettingsPanel>

<TextModal bind:open={promptOpen} title={promptOf ? `${promptOf.name}: prompt` : 'Prompt'} eyebrow="Reply profile" text={promptOf ? plainLines(promptOf.prompt_summary) : ''} {toaster} copied="Prompt copied." />

<Modal bind:open={personaOpen} title="Switch to {next?.name ?? active}?" eyebrow="Active persona" narrow>
  <p>Identity, default behaviour and staging change together.</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button
      class="btn btn--primary"
      type="button"
      onclick={() => {
        close();
        void usePersona();
      }}>Use {next?.name ?? active}</button
    >
  {/snippet}
</Modal>

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

<style>
  .persona__active .field select {
    width: 12.5rem;
  }

  .persona__effective {
    flex: 1 1 14rem;
    margin: 0;
  }

  .persona__profiles {
    flex: 1 0 auto;
  }

  .profiles__tools {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .profiles__search {
    width: 13.75rem;
    min-height: 2.125rem;
  }

  .profiles__count {
    font-family: var(--mono);
  }

  .profiles__selected {
    margin-left: auto;
    font-size: var(--fs-small);
  }

  .profiles__wrap {
    min-width: 0;
    overflow-x: auto;
  }

  .profiles__pick {
    width: 1.75rem;
  }

  .profiles__pick input {
    width: 1.1rem;
    height: 1.1rem;
    margin: 0;
    accent-color: var(--accent-fill);
  }

  .profiles__table .profiles__name {
    width: 8rem;
    font-weight: 700;
  }

  .profiles__voice {
    width: 12.5rem;
  }

  .profiles__vis {
    width: 5.6rem;
  }

  .profiles__act {
    width: 7rem;
    text-align: right;
  }

  .profile__prompt {
    max-width: 1px;
  }

  .profile__prompt--default {
    overflow: hidden;
    color: var(--dim-text);
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .profile__visibility {
    min-height: 28px;
    padding: 0 0.75rem;
    border-radius: 14px;
    font-size: var(--fs-mini);
  }

  .profile__confirm-list {
    margin: 0.4rem 0 0;
    padding-left: 1.2rem;
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

  @media (max-width: 1199px) {
    .profiles__tools {
      flex-wrap: wrap;
    }
  }

  @media (max-width: 899px) {
    .profiles__voice {
      display: none;
    }

    .profiles__selected {
      flex: 1 1 100%;
      margin-left: 0;
    }
  }
</style>
