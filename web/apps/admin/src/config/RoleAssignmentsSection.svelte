<script lang="ts">
  import { tick } from 'svelte';
  import type { ConfigView, ReplyProfile, Role, RoleProfileWrite } from '@kanade/api-types';
  import { PendingLabel, Select } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import type { Change } from './dirty';
  import type { SaveRoleProfiles } from './save';

  let {
    profiles,
    assignments,
    digest,
    roles,
    rolesLoading,
    rolesError,
    refresh,
    refreshRoles,
    saveRoleProfiles,
    form,
    changes = $bindable([]),
    saving = $bindable(false),
    blocked = $bindable(false),
  }: {
    profiles: ReplyProfile[];
    assignments: ConfigView['persona']['role_profiles'];
    digest: string;
    roles: Role[] | null;
    rolesLoading: boolean;
    rolesError: string;
    refresh: () => Promise<ConfigView | null>;
    refreshRoles: () => Promise<void>;
    saveRoleProfiles: SaveRoleProfiles;
    /** The id the Persona save bar submits. */
    form: string;
    /** For the save bar: the draft's changes, a save in flight, and whether saving is blocked. */
    changes?: Change[];
    saving?: boolean;
    blocked?: boolean;
  } = $props();
  const uid = $props.id();

  let roleSearch = $state('');
  let addRoleId = $state('');
  let addProfile = $state('');
  let roleDraft = $state<RoleProfileWrite[]>([]);
  let savedRoleAssignments = $state<RoleProfileWrite[]>([]);
  let savedRoleDigest = $state('');
  let roleStatus = $state('');
  let roleError = $state('');
  let roleSaving = $state(false);
  let roleRecovering = $state(false);
  let roleConflict = $state(false);
  let roleSearchInput: HTMLInputElement | undefined = $state();
  let roleReloadButton: HTMLButtonElement | undefined = $state();
  let roleStatusElement: HTMLParagraphElement | undefined = $state();
  let roleRemoveButtons: (HTMLButtonElement | undefined)[] = [];
  let roleHydratedKey = '';

  const roleConfigKey = $derived(
    JSON.stringify({
      digest,
      assignments: assignments.map(({ role_id, profile }) => ({ role_id, profile })),
    }),
  );
  const roleDraftChanged = $derived(!sameRoleAssignments(roleDraft, savedRoleAssignments));
  const availableRoles = $derived(
    (roles ?? []).filter(
      (role) =>
        !roleDraft.some((assignment) => assignment.role_id === role.id) &&
        role.name.toLowerCase().includes(roleSearch.trim().toLowerCase()),
    ),
  );
  const canSaveRoleDraft = $derived(
    !roleSaving && !roleRecovering && !rolesLoading && !roleConflict && roleDraftChanged && roleDraft.every((assignment) => {
      const saved = savedRoleAssignments.find((item) => item.role_id === assignment.role_id);
      const currentRole = roles?.some((role) => role.id === assignment.role_id) ?? false;
      if (!saved) return currentRole;
      return currentRole || saved.profile === assignment.profile;
    }),
  );
  const roleDraftAllowed = $derived(
    roleDraft.every((assignment, index) => roleDraft.findIndex((item) => item.role_id === assignment.role_id) === index),
  );

  const profileName = (key: string) => profiles.find((p) => p.key === key)?.name ?? key;
  const profileOptions = $derived(profiles.map((p) => ({ value: p.key, label: p.name })));
  const label = (id: string) => roleIdentity(id).label;
  // The save bar's summary: added, removed and re-profiled roles, else the new order.
  $effect(() => {
    const before = new Map(savedRoleAssignments.map((a) => [a.role_id, a.profile]));
    const after = new Map(roleDraft.map((a) => [a.role_id, a.profile]));
    const list: Change[] = [];
    for (const [id, profile] of after) {
      if (!before.has(id)) list.push({ label: label(id), from: 'none', to: profileName(profile) });
      else if (before.get(id) !== profile) list.push({ label: label(id), from: profileName(before.get(id)!), to: profileName(profile) });
    }
    for (const [id, profile] of before) if (!after.has(id)) list.push({ label: label(id), from: profileName(profile), to: 'removed' });
    if (!list.length && roleDraftChanged)
      list.push({ label: 'Order', from: savedRoleAssignments.map((a) => label(a.role_id)).join(' › '), to: roleDraft.map((a) => label(a.role_id)).join(' › ') });
    changes = list;
  });
  $effect(() => {
    saving = roleSaving;
    blocked = !canSaveRoleDraft || !roleDraftAllowed;
  });

  /** The save bar's Discard: back to the saved assignments. */
  export function discard() {
    if (roleSaving || roleRecovering) return;
    adoptRoleAssignments(assignments, digest);
  }

  $effect(() => {
    if (!profiles.some((profile) => profile.key === addProfile))
      addProfile = profiles.find((profile) => profile.public)?.key ?? profiles[0]?.key ?? '';
  });

  $effect(() => {
    const key = roleConfigKey;
    if (key === roleHydratedKey) return;
    roleHydratedKey = key;
    adoptRoleAssignments(assignments, digest);
  });

  function sameRoleAssignments(a: RoleProfileWrite[], b: RoleProfileWrite[]): boolean {
    return a.length === b.length && a.every((item, index) => item.role_id === b[index]?.role_id && item.profile === b[index]?.profile);
  }

  function roleAssignments(source: ConfigView['persona']['role_profiles']): RoleProfileWrite[] {
    return source.map(({ role_id, profile }) => ({ role_id, profile }));
  }

  function adoptRoleAssignments(source: ConfigView['persona']['role_profiles'], nextDigest: string) {
    const saved = roleAssignments(source);
    savedRoleAssignments = saved;
    roleDraft = saved.map((assignment) => ({ ...assignment }));
    savedRoleDigest = nextDigest;
    roleError = '';
    roleStatus = '';
    roleConflict = false;
  }

  function findRole(id: string): Role | undefined {
    return roles?.find((role) => role.id === id);
  }

  function roleName(role: Role): string {
    const name = `@${role.name.replace(/^@/, '')}`;
    const duplicates = roles?.filter((each) => each.name.toLocaleLowerCase() === role.name.toLocaleLowerCase()).length ?? 0;
    return duplicates > 1 ? `${name} · ID ${role.id}` : name;
  }

  function roleIdentity(id: string): { label: string; detail: string } {
    const role = findRole(id);
    if (role) return { label: roleName(role), detail: '' };
    if (rolesLoading) return { label: 'Checking saved role', detail: `ID ${id}` };
    if (rolesError) return { label: 'Role status unknown', detail: `ID ${id}` };
    return { label: 'Unavailable role', detail: `ID ${id}` };
  }

  function roleOptionsFor(index: number): Role[] {
    const selectedId = roleDraft[index]?.role_id;
    const needle = roleSearch.trim().toLowerCase();
    return (roles ?? []).filter(
      (role) =>
        !roleDraft.some((assignment, other) => other !== index && assignment.role_id === role.id) &&
        (role.id === selectedId || !needle || role.name.toLowerCase().includes(needle)),
    );
  }

  function focusRoleContext() {
    if (roleSearchInput && !roleSearchInput.disabled) roleSearchInput.focus();
    else roleStatusElement?.focus();
  }

  async function retryRoleDirectory() {
    await refreshRoles();
    await tick();
    if (!rolesError) focusRoleContext();
  }

  function changeRole(index: number, id: string) {
    const role = findRole(id);
    const current = roleDraft[index];
    if (!role || !current || roleDraft.some((assignment, other) => other !== index && assignment.role_id === id)) return;
    roleDraft[index] = { role_id: role.id, profile: current.profile };
    roleStatus = `Changed the role for assignment ${index + 1} to ${roleName(role)}.`;
    roleError = '';
  }

  function changeRoleProfile(index: number, profile: string) {
    if (!profiles.some((item) => item.key === profile) || !roleDraft[index]) return;
    roleDraft[index] = { ...roleDraft[index]!, profile };
    roleStatus = '';
    roleError = '';
  }

  async function addRoleAssignment(event: SubmitEvent) {
    event.preventDefault();
    const role = findRole(addRoleId);
    if (!role || !profiles.some((profile) => profile.key === addProfile) || roleDraft.some((assignment) => assignment.role_id === role.id)) return;
    roleDraft.push({ role_id: role.id, profile: addProfile });
    addRoleId = '';
    roleError = '';
    roleStatus = `Added ${roleName(role)} to this draft.`;
    await tick();
    roleSearchInput?.focus();
  }

  function moveRoleAssignment(index: number, delta: -1 | 1) {
    const target = index + delta;
    if (target < 0 || target >= roleDraft.length) return;
    const identity = roleIdentity(roleDraft[index]!.role_id);
    [roleDraft[index], roleDraft[target]] = [roleDraft[target]!, roleDraft[index]!];
    roleStatus = `Moved ${identity.label} to position ${target + 1} of ${roleDraft.length}.`;
    roleError = '';
  }

  async function removeRoleAssignment(index: number) {
    const removed = roleDraft[index];
    if (!removed) return;
    const identity = roleIdentity(removed.role_id);
    roleDraft.splice(index, 1);
    roleStatus = `Removed ${identity.label} from this draft.`;
    roleError = '';
    await tick();
    const focusIndex = Math.min(index, roleDraft.length - 1);
    if (focusIndex >= 0) roleRemoveButtons[focusIndex]?.focus();
    else focusRoleContext();
  }

  async function saveRoleAssignments() {
    if (!canSaveRoleDraft || !roleDraftAllowed) return;
    roleSaving = true;
    roleError = '';
    roleStatus = 'Saving role assignments…';
    const result = await saveRoleProfiles(roleDraft.map(({ role_id, profile }) => ({ role_id, profile })), savedRoleDigest);
    roleSaving = false;
    if (!result.ok) {
      roleError = result.message;
      roleConflict = result.status === 409 || result.code === 'conflict';
      roleStatus = roleConflict ? 'Your draft is still here. Reload the latest assignments to recover from this conflict.' : '';
      if (roleConflict) {
        await tick();
        roleReloadButton?.focus();
      }
      return;
    }
    adoptRoleAssignments(result.value.persona.role_profiles, result.value.persona.role_profiles_digest);
    roleStatus = 'Role assignments saved.';
    await tick();
    focusRoleContext();
  }

  async function reloadLatestRoleAssignments() {
    if (roleSaving || roleRecovering) return;
    roleRecovering = true;
    roleError = '';
    roleStatus = '';
    const latest = await refresh();
    if (!latest) {
      roleRecovering = false;
      roleError = 'Could not reload the latest assignments. Your draft is unchanged.';
      await tick();
      roleReloadButton?.focus();
      return;
    }
    adoptRoleAssignments(latest.persona.role_profiles, latest.persona.role_profiles_digest);
    await refreshRoles();
    roleRecovering = false;
    roleStatus = 'Latest saved assignments loaded. Any unsaved draft was replaced by this explicit reload.';
    await tick();
    focusRoleContext();
  }
</script>

<section class="role-profile" aria-labelledby="{uid}-heading">
  <h4 id="{uid}-heading" class="vh">Reply profile per Discord role</h4>
  <p class="settings__box">
    The first matching role in this order overrides a member's saved reply style. Role assignments never grant chatbot access.
  </p>
  {#if rolesLoading}
    <p class="note" role="status">Loading the current guild roles…</p>
  {:else if rolesError}
    <div class="settings__box settings__box--risk role-profile__directory-error">
      <p class="field__error" role="alert">Couldn't load current guild roles: {rolesError} Saved role names are temporarily unknown; you can still remove or reorder assignments.</p>
      <button class="btn" type="button" disabled={roleSaving || roleRecovering} onclick={() => void retryRoleDirectory()}>Retry current guild roles</button>
    </div>
  {:else if roles?.length === 0}
    <p class="note" role="status">No current guild roles are available. You can still remove or reorder saved assignments.</p>
  {:else if roles === null}
    <p class="note" role="status">Checking the current guild role list…</p>
  {:else}
    <p class="note" role="status">Current guild roles are loaded. New and changed assignments use this list.</p>
  {/if}

  <div class="settings__card role-profile__card" data-fid="cfg-card">
    <div class="role-profile__cols cap" aria-hidden="true"><span>#</span><span>Discord role</span><span>Reply profile</span><span>Order</span><span></span></div>
    <ol class="role-profile__list" aria-label="Role assignments in precedence order">
      {#each roleDraft as assignment, index (assignment.role_id)}
        {@const currentRole = findRole(assignment.role_id)}
        {@const identity = roleIdentity(assignment.role_id)}
        {@const canEdit = !!currentRole && !rolesLoading && !rolesError && !roleSaving && !roleRecovering && !roleConflict}
        <li class="role-profile__row" class:role-profile__row--missing={!currentRole && !rolesLoading}>
          <span class="role-profile__position">{index + 1}</span>
          {#if currentRole}
            <span class="role-profile__identity">
              <strong class="vh">{identity.label}</strong>
              <Select
                class="role-profile__select"
                label="Discord role"
                value={assignment.role_id}
                options={roleOptionsFor(index).map((role) => ({ value: role.id, label: roleName(role) }))}
                disabled={!canEdit}
                noun="roles"
                onchange={(id) => changeRole(index, id)}
              />
            </span>
          {:else}
            <span class="role-profile__identity role-profile__identity--missing">
              <span class="role-profile__bang" aria-hidden="true">!</span>
              <span
                ><strong>{identity.label}</strong>
                {#if identity.detail}<code class="role-profile__id">{identity.detail}</code>{/if}
                {#if !rolesLoading}
                  <span class="role-profile__identity-note"
                    >{rolesError ? 'Temporarily unknown; this saved assignment can only be removed or reordered.' : 'No longer in the guild; this saved assignment can only be removed or reordered.'}</span
                  >
                {/if}</span
              >
            </span>
          {/if}
          <Select label="Reply profile" value={assignment.profile} options={profileOptions} disabled={!canEdit} noun="profiles" onchange={(key) => changeRoleProfile(index, key)} />
          <span class="role-profile__order">
            <button class="btn role-profile__icon" type="button" aria-label="Move up" disabled={index === 0 || roleSaving || roleRecovering || roleConflict} onclick={() => moveRoleAssignment(index, -1)}
              ><span aria-hidden="true">↑</span></button
            >
            <button
              class="btn role-profile__icon"
              type="button"
              aria-label="Move down"
              disabled={index === roleDraft.length - 1 || roleSaving || roleRecovering || roleConflict}
              onclick={() => moveRoleAssignment(index, 1)}><span aria-hidden="true">↓</span></button
            >
          </span>
          <button
            class="btn btn--ghost role-profile__icon role-profile__remove"
            type="button"
            bind:this={roleRemoveButtons[index]}
            aria-label="Remove {identity.label} assignment"
            disabled={roleSaving || roleRecovering || roleConflict}
            onclick={() => void removeRoleAssignment(index)}><span aria-hidden="true">×</span></button
          >
        </li>
      {:else}
        <li class="note role-profile__empty">No role assignments; member selections and the persona default apply.</li>
      {/each}
    </ol>

    <form class="role-profile__add" onsubmit={addRoleAssignment} aria-label="Add an assignment">
      <input
        class="role-profile__search"
        bind:this={roleSearchInput}
        type="search"
        bind:value={roleSearch}
        aria-label="Search current guild roles"
        disabled={rolesLoading || !!rolesError || roles === null}
        placeholder="find a role…"
      />
      <Select
        label="New assignment role"
        bind:value={addRoleId}
        options={availableRoles.map((role) => ({ value: role.id, label: roleName(role) }))}
        placeholder="Choose a role…"
        noun="roles"
        disabled={rolesLoading || !!rolesError || roles === null || availableRoles.length === 0}
      />
      <Select
        label="Reply profile"
        bind:value={addProfile}
        options={profileOptions}
        noun="profiles"
        disabled={!profiles.length || rolesLoading || !!rolesError || roles === null || roles.length === 0}
      />
      <button class="btn" type="submit" disabled={roleSaving || roleRecovering || roleConflict || !addRoleId || rolesLoading || !!rolesError || !roles?.length}>Add assignment</button>
    </form>
  </div>
  <!-- The Persona save bar submits this (form=); the rows above are the draft. -->
  <form id={form} class="vh" aria-hidden="true" onsubmit={(event) => { event.preventDefault(); void saveRoleAssignments(); }}></form>

  {#if roleError}<p class="field__error" role="alert">{roleError}</p>{/if}
  {#if roleConflict}
    <div class="settings__box settings__box--warn role-profile__recovery">
      <p class="note">Reloading replaces this unsaved draft with the latest saved assignments. Your draft remains unchanged until the reload succeeds.</p>
      <button class="btn" type="button" bind:this={roleReloadButton} disabled={roleRecovering} onclick={() => void reloadLatestRoleAssignments()}>
        <PendingLabel pending={roleRecovering} label="Reloading…">Reload latest assignments</PendingLabel>
      </button>
    </div>
  {/if}
  <p class="note role-profile__status" role="status" aria-live="polite" tabindex="-1" bind:this={roleStatusElement}>{roleStatus}</p>
</section>

<style>
  .role-profile {
    display: grid;
    gap: 0.875rem;
  }

  .role-profile > .note {
    margin: 0;
  }

  .role-profile__directory-error,
  .role-profile__recovery {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 0.75rem;
  }

  .role-profile__directory-error .field__error,
  .role-profile__recovery .note {
    flex: 1 1 18rem;
    margin: 0;
  }

  .role-profile__card {
    gap: 0.5rem;
  }

  /* Board B_CfgRoles: # · Discord role · Reply profile · Order · remove. */
  .role-profile__cols,
  .role-profile__row,
  .role-profile__add {
    display: grid;
    grid-template-columns: 1.75rem minmax(0, 1fr) 12.5rem 6rem 2.25rem;
    align-items: center;
    gap: 0.625rem;
  }

  .role-profile__cols {
    padding: 0 0.5rem;
  }

  .role-profile__list {
    display: grid;
    gap: 0.125rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .role-profile__row {
    padding: 0.375rem 0.5rem;
    border-radius: 6px;
    background: var(--row);
  }

  .role-profile__row--missing {
    background: color-mix(in srgb, var(--risk) 7%, var(--row));
  }

  .role-profile__add input {
    width: 100%;
    min-width: 0;
  }

  .role-profile__identity {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 0.5rem;
  }

  .role-profile__identity :global(.ddw) {
    flex: 1 1 auto;
  }

  .role-profile__bang {
    flex: none;
    color: color-mix(in srgb, var(--risk-text) 80%, var(--ink));
    font-weight: 700;
  }

  .role-profile__position {
    font-family: var(--mono);
    font-weight: 700;
  }

  .role-profile__id,
  .role-profile__identity-note {
    color: color-mix(in srgb, var(--risk-text) 80%, var(--ink));
    font-size: var(--fs-mini);
  }

  .role-profile__identity-note::before {
    content: "· ";
  }

  .role-profile__order {
    display: flex;
    gap: 0.25rem;
  }

  .role-profile__icon {
    justify-content: center;
    width: 2rem;
    min-width: 2rem;
    min-height: 2rem;
    padding: 0;
  }

  .role-profile__remove {
    color: color-mix(in srgb, var(--risk-text) 80%, var(--ink));
  }

  .role-profile__add {
    grid-template-columns: minmax(0, 0.8fr) minmax(0, 1fr) 12.5rem auto;
    margin-top: 0.375rem;
    padding: 0.375rem 0.5rem;
    border-radius: 16px;
    box-shadow: inset 0 0 0 1.5px var(--line);
  }

  .role-profile__empty {
    padding: 0.5rem;
  }

  .role-profile__status:empty {
    display: none;
  }

  /* $single-pane in @kanade/ui _breakpoints.scss (plain CSS here). */
  @media (width < 900px) {
    .role-profile__cols {
      display: none;
    }

    .role-profile__row {
      grid-template-columns: 1.5rem minmax(0, 1fr) auto auto;
    }

    .role-profile__row > .role-profile__identity {
      grid-column: 2 / -1;
    }

    .role-profile__row > :global(.ddw) {
      grid-column: 2;
    }

    .role-profile__add {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }

    .role-profile__search,
    .role-profile__add > .btn {
      grid-column: 1 / -1;
    }

    .role-profile__add > .btn {
      min-height: 2.75rem;
    }
  }
</style>
