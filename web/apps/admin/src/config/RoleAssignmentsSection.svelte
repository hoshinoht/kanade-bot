<script lang="ts">
  import { tick } from 'svelte';
  import type { ConfigView, ReplyProfile, Role, RoleProfileWrite } from '@kanade/api-types';
  import { PendingLabel } from '@kanade/ui';
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
  let roleSaveButton: HTMLButtonElement | undefined = $state();
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
      } else {
        await tick();
        roleSaveButton?.focus();
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
  <h4 id="{uid}-heading" class="settings__subtitle">Reply profile per Discord role</h4>
  <p class="note">
    The first matching readable role in this order overrides a member's saved reply style. Role assignments do not grant chatbot access.
  </p>
  {#if rolesLoading}
    <p class="note" role="status">Loading the current guild roles…</p>
  {:else if rolesError}
    <div class="role-profile__directory-error">
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

  <form class="role-profile__add" onsubmit={addRoleAssignment}>
    <label class="field role-profile__search">
      <span>Search current guild roles</span>
      <input bind:this={roleSearchInput} type="search" bind:value={roleSearch} disabled={rolesLoading || !!rolesError || roles === null} placeholder="role name" />
    </label>
    <label class="field">
      <span>New assignment role</span>
      <select bind:value={addRoleId} disabled={rolesLoading || !!rolesError || roles === null || availableRoles.length === 0}>
        <option value="">Choose a role…</option>
        {#each availableRoles as role (role.id)}<option value={role.id}>{roleName(role)}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span>Reply profile</span>
      <select bind:value={addProfile} disabled={!profiles.length || rolesLoading || !!rolesError || roles === null || roles.length === 0}>
        {#each profiles as profile (profile.key)}<option value={profile.key}>{profile.name}</option>{/each}
      </select>
    </label>
    <button class="btn btn--primary" type="submit" disabled={roleSaving || roleRecovering || roleConflict || !addRoleId || rolesLoading || !!rolesError || !roles?.length}>Add assignment</button>
  </form>

  <ol class="role-profile__list" aria-label="Role assignments in precedence order">
    {#each roleDraft as assignment, index (assignment.role_id)}
      {@const currentRole = findRole(assignment.role_id)}
      {@const identity = roleIdentity(assignment.role_id)}
      {@const canEdit = !!currentRole && !rolesLoading && !rolesError && !roleSaving && !roleRecovering && !roleConflict}
      <li class="role-profile__row">
        <div class="role-profile__identity">
          <span class="role-profile__position">{index + 1}</span>
          <div>
            <strong>{identity.label}</strong>
            {#if identity.detail}<code class="role-profile__id">{identity.detail}</code>{/if}
            {#if !currentRole && !rolesLoading}
              <span class="note role-profile__identity-note">{rolesError ? 'Temporarily unknown; this saved assignment can only be removed or reordered.' : 'Not in the current guild role list; this saved assignment can only be removed or reordered.'}</span>
            {/if}
          </div>
        </div>
        <label class="field">
          <span>Discord role</span>
          {#if currentRole}
            <select
              class="role-profile__select"
              value={assignment.role_id}
              disabled={!canEdit}
              onchange={(event) => changeRole(index, event.currentTarget.value)}
            >
              {#each roleOptionsFor(index) as role (role.id)}<option value={role.id}>{roleName(role)}</option>{/each}
            </select>
          {:else}
            <select class="role-profile__select" disabled aria-label="Discord role">
              <option>{rolesLoading ? 'Checking current roles…' : rolesError ? 'Role status unknown' : 'Unavailable role — cannot change'}</option>
            </select>
          {/if}
        </label>
        <label class="field">
          <span>Reply profile</span>
          <select value={assignment.profile} disabled={!canEdit} onchange={(event) => changeRoleProfile(index, event.currentTarget.value)}>
            {#each profiles as profile (profile.key)}<option value={profile.key}>{profile.name}</option>{/each}
          </select>
        </label>
        <div class="role-profile__actions" aria-label="Assignment order and removal">
          <button class="btn" type="button" disabled={index === 0 || roleSaving || roleRecovering || roleConflict} onclick={() => moveRoleAssignment(index, -1)}>Move up</button>
          <button class="btn" type="button" disabled={index === roleDraft.length - 1 || roleSaving || roleRecovering || roleConflict} onclick={() => moveRoleAssignment(index, 1)}>Move down</button>
          <button
            class="btn btn--ghost"
            type="button"
            bind:this={roleRemoveButtons[index]}
            aria-label="Remove {identity.label} assignment"
            disabled={roleSaving || roleRecovering || roleConflict}
            onclick={() => void removeRoleAssignment(index)}>Remove</button
          >
        </div>
      </li>
    {:else}
      <li class="note role-profile__empty">No role assignments; member selections and the persona default apply.</li>
    {/each}
  </ol>

  {#if roleError}<p class="field__error" role="alert">{roleError}</p>{/if}
  {#if roleConflict}
    <div class="role-profile__recovery">
      <p class="note">Reloading replaces this unsaved draft with the latest saved assignments. Your draft remains unchanged until the reload succeeds.</p>
      <button class="btn" type="button" bind:this={roleReloadButton} disabled={roleRecovering} onclick={() => void reloadLatestRoleAssignments()}>
        <PendingLabel pending={roleRecovering} label="Reloading…">Reload latest assignments</PendingLabel>
      </button>
    </div>
  {/if}
  {#if roleDraftChanged}
    <div class="settings__actions role-profile__save">
      <button class="btn btn--primary" type="button" bind:this={roleSaveButton} disabled={!canSaveRoleDraft || !roleDraftAllowed} onclick={() => void saveRoleAssignments()}>
        <PendingLabel pending={roleSaving} label="Saving…">Save role assignments</PendingLabel>
      </button>
      {#if !roleConflict}<button class="btn btn--ghost" type="button" disabled={roleSaving || roleRecovering} onclick={() => adoptRoleAssignments(assignments, digest)}>Discard draft</button>{/if}
    </div>
  {/if}
  <p class="note role-profile__status" role="status" aria-live="polite" tabindex="-1" bind:this={roleStatusElement}>{roleStatus}</p>
</section>

<style>
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

  .role-profile__add {
    display: grid;
    grid-template-columns: minmax(10rem, 1.2fr) minmax(10rem, 1fr) minmax(10rem, 1fr) auto;
    align-items: end;
    gap: 0.5rem 0.65rem;
    margin: 0.75rem 0;
  }

  .role-profile__add .field {
    min-width: 0;
    margin: 0;
  }

  .role-profile__add input,
  .role-profile__add select,
  .role-profile__row select {
    width: 100%;
    min-width: 0;
  }

  .role-profile__list {
    display: grid;
    gap: 0.15rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .role-profile__row {
    display: grid;
    grid-template-columns: minmax(9rem, 1.35fr) minmax(9rem, 1fr) minmax(9rem, 1fr) auto;
    align-items: end;
    gap: 0.55rem 0.7rem;
    padding: 0.7rem 0;
    border-bottom: 1px solid var(--line-soft);
  }

  .role-profile__identity {
    display: flex;
    min-width: 0;
    align-items: flex-start;
    gap: 0.55rem;
    align-self: center;
  }

  .role-profile__identity > div {
    min-width: 0;
  }

  .role-profile__identity strong {
    display: block;
  }

  .role-profile__position {
    flex: none;
    min-width: 1.6rem;
    color: var(--dim-text);
    font-family: var(--mono);
    font-weight: 700;
    text-align: right;
  }

  .role-profile__id,
  .role-profile__identity-note {
    display: block;
    margin-top: 0.2rem;
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  .role-profile__identity-note {
    line-height: 1.35;
  }

  .role-profile__actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }

  .role-profile__actions .btn,
  .role-profile__add > .btn,
  .role-profile__save .btn,
  .role-profile__directory-error .btn,
  .role-profile__recovery .btn {
    min-height: 2.75rem;
    white-space: nowrap;
  }

  .role-profile__empty {
    padding: 0.7rem 0;
    border-bottom: 1px solid var(--line-soft);
  }

  .role-profile__save {
    margin: 0.7rem 0 0;
  }

  .role-profile__status:empty {
    display: none;
  }

  @media (max-width: 1099px) {
    .role-profile__add {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }

    .role-profile__search,
    .role-profile__add > .btn {
      grid-column: 1 / -1;
    }

    .role-profile__row {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    }

    .role-profile__identity,
    .role-profile__actions {
      grid-column: 1 / -1;
    }
  }

  @media (max-width: 599px) {
    .role-profile__row {
      column-gap: 0.45rem;
    }

    .role-profile__actions {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }

    .role-profile__actions .btn {
      padding-inline: 0.35rem;
      white-space: normal;
    }
  }
</style>
