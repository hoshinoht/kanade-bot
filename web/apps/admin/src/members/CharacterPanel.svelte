<!--
  The member sheet's Characters tab (module `roster`; boards B_RosterChars,
  B_PhoneRosterChars, B_RosterStates): a summary and Add, the active
  characters (main first, crowned), the retired ones dimmed in their own
  group, each with Edit and a More menu (Make main / Retire or Bring back,
  Delete…). Retire, Bring back and Make main undo from a toast; Delete is
  confirmed. The main's Retire and Delete wait, explained, while other
  characters are active. Characters stay in the admin app: never model input.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { CharacterRow, MemberCharacters, MemberRow } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState, StateNote, type Toaster } from '@kanade/ui';
  import { memberLabel } from '../names/directory.svelte';
  import { actorName } from '../history/describe';
  import { send, type Resource } from '../resource.svelte';
  import CharacterDialog from './CharacterDialog.svelte';
  import DeleteCharacter from './DeleteCharacter.svelte';
  import { dayMonth, DEFAULT_REGION, mainLocked, mainOf, regionLabel, splitCharacters } from './characters';
  import { dialogLayer } from './layer.svelte';

  let {
    member,
    memberName,
    members,
    characters,
    phone,
    timeZone,
    toaster,
  }: {
    member: MemberRow;
    /** The member as the page names them ("Mika", "Ren (2)"). */
    memberName: string;
    members: MemberRow[];
    characters: Resource<MemberCharacters>;
    /** The phone frame: 44 px targets, bottom sheets. */
    phone: boolean;
    timeZone: string;
    toaster: Toaster;
  } = $props();

  const uid = $props.id();
  // Add/Edit and Delete open over the page (MembersPage mounts them), not inside this pane.
  const layer = dialogLayer();
  $effect(() => layer.hold(dialogs));
  const rows = $derived(characters.data?.member_id === member.id ? characters.data.characters : null);
  const groups = $derived(splitCharacters(rows ?? []));

  let menuFor = $state<string | null>(null);
  let menu = $state<HTMLDivElement>();
  let editing = $state<CharacterRow | null>(null);
  let dialogOpen = $state(false);
  let editorMounted = $state(false);
  let deleting = $state<CharacterRow | null>(null);
  let deleteOpen = $state(false);
  let deleteBusy = $state(false);
  let deleteError = $state('');
  let panel = $state<HTMLElement>();
  /** Where focus lands after a delete: the next row's Edit, else Add. */
  let afterDelete: string | null = null;

  const names = (id: string) => memberLabel(members, id);
  const known = (id: string) => members.some((m) => m.id === id);
  const updatedBy = (row: CharacterRow) => actorName(row.updated_by, names, known);

  async function refresh(): Promise<CharacterRow[]> {
    await characters.refresh();
    return characters.data?.characters ?? [];
  }

  /** After the list re-renders, focus this character's control (`edit` or `more`), else Add. */
  async function focusRow(id: string | null, part: 'edit' | 'more') {
    await tick();
    const target = (id && panel?.querySelector<HTMLElement>(`[data-character="${id}"] .rosterrow__${part}`)) || panel?.querySelector<HTMLElement>('.rosterpanel__add');
    target?.focus({ preventScroll: true });
  }

  type Change = { retired?: boolean; role?: 'main' };

  /** One menu write; `undo` names the change that puts it back. */
  async function change(row: CharacterRow, body: Change, done: string, undo: (() => Promise<void>) | null) {
    const result = await send((c) => c.patch<CharacterRow>(`/api/admin/characters/${encodeURIComponent(row.id)}`, { version: row.version, ...body }));
    await refresh();
    if (!result.ok) {
      toaster.show({
        message: `Couldn't change ${row.ign}: ${result.message}`,
        tone: 'error',
        action: { label: 'Try again', run: () => void retry(row.id, body, done, undo) },
      });
      return false;
    }
    toaster.show(undo ? { message: done, tone: 'ok', action: { label: 'Undo', run: () => void undo() } } : { message: done, tone: 'ok' });
    return true;
  }

  /** Again, at the version now listed. */
  async function retry(id: string, body: Change, done: string, undo: (() => Promise<void>) | null) {
    const row = (await refresh()).find((c) => c.id === id);
    if (row) await change(row, body, done, undo);
  }

  /** Undo puts back the state recorded at the change, at the version listed now. */
  async function revert(id: string, body: Change, done: string) {
    const row = (await refresh()).find((c) => c.id === id);
    if (row) await change(row, body, done, null);
  }

  async function retire(row: CharacterRow) {
    if (await change(row, { retired: true }, `Retired ${row.ign}.`, () => revert(row.id, { retired: false }, `Brought back ${row.ign}.`))) void focusRow(row.id, 'more');
  }

  async function bringBack(row: CharacterRow) {
    if (await change(row, { retired: false }, `Brought back ${row.ign}.`, () => revert(row.id, { retired: true }, `Retired ${row.ign}.`))) void focusRow(row.id, 'more');
  }

  async function makeMain(row: CharacterRow) {
    const before = mainOf(rows ?? []);
    const undo = before ? () => revert(before.id, { role: 'main' }, `${before.ign} is ${memberName}'s main again.`) : null;
    if (await change(row, { role: 'main' }, `${row.ign} is ${memberName}'s main now.`, undo)) void focusRow(row.id, 'more');
  }

  function askDelete(row: CharacterRow) {
    const all = [...groups.active, ...groups.retired];
    const at = all.findIndex((c) => c.id === row.id);
    afterDelete = (all[at + 1] ?? all[at - 1])?.id ?? null;
    deleteError = '';
    deleting = row;
    deleteOpen = true;
  }

  async function confirmDelete() {
    const row = deleting!;
    deleteBusy = true;
    const result = await send((c) => c.delete<{ message: string }>(`/api/admin/characters/${encodeURIComponent(row.id)}`));
    deleteBusy = false;
    if (!result.ok) {
      deleteError =
        result.code === 'main_required'
          ? `${row.ign} is ${memberName}'s main now, and the main stays while other characters are active. Make another character main first.`
          : `Couldn't delete ${row.ign}: ${result.message}`;
      void refresh();
      return;
    }
    await refresh();
    toaster.show({ message: `Deleted ${row.ign}. History keeps the audit row.`, tone: 'ok' });
    deleteOpen = false;
  }

  async function retireInstead() {
    const row = deleting!;
    deleteBusy = true;
    const done = await change(row, { retired: true }, `Retired ${row.ign}.`, () => revert(row.id, { retired: false }, `Brought back ${row.ign}.`));
    deleteBusy = false;
    if (done) deleteOpen = false;
    else deleteError = `Couldn't retire ${row.ign}.`;
  }

  function openDialog(row: CharacterRow | null) {
    menuFor = null;
    editing = row;
    dialogOpen = true;
    editorMounted = true;
  }

  // Menu (APG menu button, as NavMore): ↓/Enter/Space open on the first item,
  // ↑ on the last; arrows wrap, Home/End jump, Escape returns to the button.
  const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

  async function showMenu(id: string, at: 'first' | 'last') {
    menuFor = id;
    await tick();
    const all = items();
    (at === 'first' ? all[0] : all[all.length - 1])?.focus();
  }

  function hideMenu(refocus = true) {
    const id = menuFor;
    menuFor = null;
    if (refocus && id) void focusRow(id, 'more');
  }

  function onButtonKey(event: KeyboardEvent, id: string) {
    if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      void showMenu(id, 'first');
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      void showMenu(id, 'last');
    }
  }

  function onMenuKey(event: KeyboardEvent) {
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    const moves: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: all.length - 1 };
    if (event.key in moves) {
      event.preventDefault();
      all[(moves[event.key]! + all.length) % all.length]?.focus();
    } else if (event.key === 'Escape') {
      // The pane's own Escape (close the member) must not fire too.
      event.preventDefault();
      event.stopPropagation();
      hideMenu();
    } else if (event.key === 'Tab') {
      hideMenu(false);
    }
  }

  $effect(() => {
    if (!menuFor) return;
    const away = (event: PointerEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest(`[data-character="${menuFor}"] .rosterrow__menu, [data-character="${menuFor}"] .rosterrow__more`)) hideMenu(false);
    };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  });

  /** A disabled-but-explained item does nothing; the menu stays open. */
  function pick(run: () => void, disabled = false) {
    if (disabled) return;
    menuFor = null;
    run();
  }
</script>

{#snippet row(c: CharacterRow, tagged: boolean)}
  {@const retired = c.retired_at !== null}
  {@const locked = mainLocked(c, rows ?? [])}
  {@const open = menuFor === c.id}
  <li class="rosterrow" class:rosterrow--retired={retired} data-character={c.id} data-fid={tagged ? 'roster-row' : undefined}>
    <span class="rosterrow__id">
      <span class="rosterrow__ign"
        >{c.ign}{#if c.role === 'main' && !retired}<span class="rosterrow__crown" data-fid={tagged ? 'roster-main' : undefined}><Icon name="crown" />main</span
          >{/if}{#if retired}<span class="rosterrow__retag">retired</span>{/if}</span
      >
      <span class="rosterrow__meta"
        >{#if c.level}<span class="rosterrow__lvmeta mono">Lv. {c.level}</span><span class="rosterrow__lvsep">&nbsp;·&nbsp;</span>{/if}{c.class ?? 'No class'}{#if c.world !== DEFAULT_REGION}{` · ${regionLabel(c.world)}`}{/if}<span
          class="rosterrow__updated">&nbsp;· updated <span class="mono">{dayMonth(c.updated_at, timeZone)}</span></span
        ></span
      >
    </span>
    <span class="rosterrow__lv mono">{#if c.level}<small>Lv.</small> {c.level}{/if}</span>
    <button type="button" class="rosterrow__btn rosterrow__edit" aria-label="Edit {c.ign}" data-fid={tagged ? 'roster-edit' : undefined} onclick={() => openDialog(c)}><Icon name="edit" /></button>
    <button
      type="button"
      class="rosterrow__btn rosterrow__more"
      class:rosterrow__btn--on={open}
      aria-label="More actions for {c.ign}"
      aria-haspopup="menu"
      aria-expanded={open}
      aria-controls={open ? `${uid}-menu` : undefined}
      data-fid={tagged ? 'roster-more' : undefined}
      onclick={() => (open ? hideMenu() : void showMenu(c.id, 'first'))}
      onkeydown={(event) => onButtonKey(event, c.id)}><Icon name="more-vertical" /></button
    >
    {#if open}
      <div class="rosterrow__menu" id="{uid}-menu" role="menu" aria-label={c.ign} tabindex="-1" data-fid="roster-menu" bind:this={menu} onkeydown={onMenuKey}>
        {#if retired}
          <button type="button" role="menuitem" tabindex="-1" class="rosterrow__item" onclick={() => pick(() => void bringBack(c))}><Icon name="rotate-ccw" />Bring back</button>
        {:else}
          {#if c.role !== 'main'}
            <button type="button" role="menuitem" tabindex="-1" class="rosterrow__item" data-fid="roster-menu-main" onclick={() => pick(() => void makeMain(c))}><Icon name="crown" />Make main</button>
          {/if}
          <button
            type="button"
            role="menuitem"
            tabindex="-1"
            class="rosterrow__item"
            aria-disabled={locked ? 'true' : undefined}
            aria-describedby={locked ? `${uid}-why` : undefined}
            data-fid="roster-menu-retire"
            onclick={() => pick(() => void retire(c), locked)}><Icon name="archive" />Retire</button
          >
        {/if}
        <span class="rosterrow__sep" role="separator"></span>
        <button
          type="button"
          role="menuitem"
          tabindex="-1"
          class="rosterrow__item rosterrow__item--risk"
          aria-disabled={locked ? 'true' : undefined}
          aria-describedby={locked ? `${uid}-why` : undefined}
          data-fid="roster-menu-delete"
          onclick={() => pick(() => askDelete(c), locked)}><Icon name="trash-2" />Delete…</button
        >
        {#if locked}<span class="rosterrow__why" id="{uid}-why" data-fid="roster-menu-why">{c.ign} is the main. Make another character main first.</span>{/if}
      </div>
    {/if}
  </li>
{/snippet}

<div class="rosterpanel" class:rosterpanel--phone={phone} data-fid="roster-panel" bind:this={panel}>
  {#if characters.error && !rows}
    <div class="rosterpanel__state" data-fid="roster-error">
      <LoadError thing="characters" reason="{characters.error} The Profile tab still works." level={3} onretry={() => void characters.load()} />
    </div>
  {:else if !rows}
    <LoadingState text="Loading characters…" />
  {:else if rows.length === 0}
    <div class="rosterpanel__state" data-fid="roster-empty">
      <StateNote icon="crown" title="No characters yet" level={3}>
        {memberName}'s first character becomes the main; add alts after it.
        {#snippet actions()}
          <button type="button" class="btn btn--primary rosterpanel__add" data-fid="roster-add" onclick={() => openDialog(null)}><Icon name="plus" />Add character</button>
        {/snippet}
      </StateNote>
    </div>
  {:else}
    <div class="rosterpanel__top">
      <p class="rosterpanel__summary" data-fid="roster-summary">
        <b class="mono">{groups.active.length}</b> active{#if groups.retired.length}
          · <b class="mono">{groups.retired.length}</b> retired{/if}
      </p>
      <button type="button" class="btn rosterpanel__add" data-fid="roster-add" onclick={() => openDialog(null)}><Icon name="plus" />Add character</button>
    </div>
    {#if groups.active.length}
      <ul class="rosterrows" aria-label="{memberName}'s characters">
        {#each groups.active as c (c.id)}{@render row(c, true)}{/each}
      </ul>
    {/if}
    {#if groups.retired.length}
      <section class="rosterpanel__retired" data-fid="roster-retired" aria-labelledby="{uid}-retired">
        <h3 class="cap" id="{uid}-retired">Retired</h3>
        <ul class="rosterrows" aria-label="{memberName}'s retired characters">
          {#each groups.retired as c (c.id)}{@render row(c, false)}{/each}
        </ul>
      </section>
    {/if}
  {/if}
  {#if rows}
    <p class="rosterpanel__note" data-fid="roster-note">Kept when {memberName} leaves the server; only an admin deletes them. Characters are never sent to the chatbot.</p>
  {/if}
</div>

{#snippet dialogs()}
  <!-- Mounted while in use: from the first open until it has closed (focus restored). -->
  {#if editorMounted}
    <CharacterDialog
      bind:open={dialogOpen}
      memberId={member.id}
      {memberName}
      character={editing}
      rows={rows ?? []}
      sheet={phone}
      {timeZone}
      {updatedBy}
      actor={(who) => actorName(who, names, known)}
      onrefresh={refresh}
      onsaved={(saved, message) => {
        toaster.show({ message, tone: 'ok' });
        void refresh().then(() => focusRow(saved.id, 'edit'));
      }}
      ondelete={(c) => {
        dialogOpen = false;
        askDelete(c);
      }}
      onclosed={() => (editorMounted = dialogOpen)}
    />
  {/if}
  {#if deleting}
    <DeleteCharacter
      bind:open={deleteOpen}
      {memberName}
      character={deleting}
      sheet={phone}
      busy={deleteBusy}
      error={deleteError}
      ondelete={() => void confirmDelete()}
      onretire={() => void retireInstead()}
      returnFocus={() => {
        const id = deleting?.id;
        // Kept (Cancel, Retire instead): its own More; deleted: the next row's Edit, else Add.
        const kept = id ? panel?.querySelector<HTMLElement>(`[data-character="${id}"] .rosterrow__more`) : null;
        const next = afterDelete ? panel?.querySelector<HTMLElement>(`[data-character="${afterDelete}"] .rosterrow__edit`) : null;
        // The dialog leaves before the row does: close, then forget the character.
        queueMicrotask(() => {
          if (!deleteOpen) deleting = null;
        });
        return kept ?? next ?? panel?.querySelector<HTMLElement>('.rosterpanel__add') ?? null;
      }}
    />
  {/if}
{/snippet}
