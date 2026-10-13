<!--
  One member's characters (module `roster`): the admin member sheet's
  Characters tab (boards B_RosterChars, B_PhoneRosterChars, B_RosterStates)
  and the portal's Account › Characters (M_Chars, M_CharsMenu, M_PhoneChars).
  A summary and Add, the active characters (main first, crowned), the retired
  ones dimmed in their own group, each with Edit and a More menu (Make main /
  Retire or Bring back, and for admins Delete…). Retire, Bring back and Make
  main undo from a toast; Delete is confirmed. The main's Retire and Delete
  wait, explained, while other characters are active. A member's rows carry
  each character's link state; members never delete ("only an admin
  deletes"). Characters are never model input.
-->
<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import type { CharacterLink, CharacterRow } from '@kanade/api-types';
  import Icon from '../components/Icon.svelte';
  import LoadError from '../components/LoadError.svelte';
  import LoadingState from '../components/LoadingState.svelte';
  import { focusMenuItem, menuButtonKey, menuKey } from '../components/menu';
  import StateNote from '../components/StateNote.svelte';
  import StatusChip from '../components/StatusChip.svelte';
  import type { Toaster } from '../components/toaster.svelte';
  import '@kanade/ui/styles/screens/members.scss';
  import { STRIP_TAGS, stripState } from './chardata';
  import CharacterDialog from './CharacterDialog.svelte';
  import DeleteCharacter from './DeleteCharacter.svelte';
  import type { CharacterChange, CharacterFids, CharacterKit } from './kit.svelte';
  import { dayMonth, DEFAULT_REGION, mainLocked, mainOf, regionLabel, splitCharacters } from './roster';

  let {
    kit,
    rows,
    failed = '',
    onretry,
    phone,
    toaster,
    layer,
    links = null,
    fids = {},
  }: {
    kit: CharacterKit;
    /** The member's characters; null while loading. */
    rows: CharacterRow[] | null;
    /** Why the list did not load, in full. */
    failed?: string;
    onretry: () => void;
    /** The phone frame: 44 px targets, bottom sheets. */
    phone: boolean;
    toaster: Toaster;
    /** Where the dialogs mount (the admin Members page, over the page); without one, after the panel. */
    layer?: { hold(dialogs: Snippet): () => void };
    /** Member: each character's link, drawn as a chip by the IGN (module `character_data`). */
    links?: Record<string, CharacterLink> | null;
    fids?: CharacterFids;
  } = $props();

  const uid = $props.id();
  const member = $derived(kit.audience === 'member');
  const memberName = $derived(kit.owner);
  $effect(() => layer?.hold(dialogs));
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

  /** Open Add (null) or Edit: the portal's deep links and its "Confirm it's you" come back through this. */
  export function edit(row: CharacterRow | null) {
    openDialog(row);
  }

  /** After the list re-renders, focus this character's control (`edit` or `more`), else Add. */
  async function focusRow(id: string | null, part: 'edit' | 'more') {
    await tick();
    const target = (id && panel?.querySelector<HTMLElement>(`[data-character="${id}"] .rosterrow__${part}`)) || panel?.querySelector<HTMLElement>('.rosterpanel__add');
    target?.focus({ preventScroll: true });
  }

  type Change = Pick<CharacterChange, 'retired' | 'role'>;

  /** One menu write; `undo` names the change that puts it back. */
  async function change(row: CharacterRow, body: Change, done: string, undo: (() => Promise<void>) | null) {
    const result = await kit.api.patch(row.id, { version: row.version, ...body });
    await kit.api.refresh();
    if (!result.ok) {
      if (result.reauth && kit.onreauth) {
        kit.onreauth({ kind: 'change', row, verb: body.role === 'main' ? 'main' : body.retired ? 'retire' : 'restore' });
        return false;
      }
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
    const row = (await kit.api.refresh()).find((c) => c.id === id);
    if (row) await change(row, body, done, undo);
  }

  /** Undo puts back the state recorded at the change, at the version listed now. */
  async function revert(id: string, body: Change, done: string) {
    const row = (await kit.api.refresh()).find((c) => c.id === id);
    if (row) await change(row, body, done, null);
  }

  const retiredWords = (ign: string) => (member ? `${ign} retired. Kept for history.` : `Retired ${ign}.`);
  const whose = $derived(member ? 'your' : `${memberName}'s`);

  async function retire(row: CharacterRow) {
    if (await change(row, { retired: true }, retiredWords(row.ign), () => revert(row.id, { retired: false }, `Brought back ${row.ign}.`))) void focusRow(row.id, 'more');
  }

  async function bringBack(row: CharacterRow) {
    if (await change(row, { retired: false }, `Brought back ${row.ign}.`, () => revert(row.id, { retired: true }, retiredWords(row.ign)))) void focusRow(row.id, 'more');
  }

  async function makeMain(row: CharacterRow) {
    const before = mainOf(rows ?? []);
    const undo = before ? () => revert(before.id, { role: 'main' }, `${before.ign} is ${whose} main again.`) : null;
    if (await change(row, { role: 'main' }, `${row.ign} is ${whose} main now.`, undo)) void focusRow(row.id, 'more');
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
    const remove = kit.api.remove!;
    deleteBusy = true;
    const result = await remove(row.id);
    deleteBusy = false;
    if (!result.ok) {
      deleteError =
        result.code === 'main_required'
          ? `${row.ign} is ${memberName}'s main now, and the main stays while other characters are active. Make another character main first.`
          : `Couldn't delete ${row.ign}: ${result.message}`;
      void kit.api.refresh();
      return;
    }
    await kit.api.refresh();
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
  async function showMenu(id: string, at: 'first' | 'last') {
    menuFor = id;
    await tick();
    focusMenuItem(menu, at);
  }

  function hideMenu(refocus = true) {
    const id = menuFor;
    menuFor = null;
    if (refocus && id) void focusRow(id, 'more');
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
  {@const link = links?.[c.id]}
  {@const state = link ? stripState(link, c) : null}
  <li class="rosterrow" class:rosterrow--retired={retired} data-character={c.id} data-fid={tagged ? 'roster-row' : undefined}>
    <span class="rosterrow__id">
      <span class="rosterrow__ign"
        >{c.ign}{#if c.role === 'main' && !retired}<span class="rosterrow__crown" data-fid={tagged ? 'roster-main' : undefined}><Icon name="crown" />main</span
          >{/if}{#if retired}<span class="rosterrow__retag">retired</span>{/if}{#if state}<StatusChip tone={STRIP_TAGS[state].tone} data-fid={tagged ? fids.linkState : undefined}
            ><Icon name={STRIP_TAGS[state].icon} />{state === 'pending' ? 'waiting for you' : STRIP_TAGS[state].label}</StatusChip
          >{/if}</span
      >
      <span class="rosterrow__meta"
        >{#if c.level}<span class="rosterrow__lvmeta mono">Lv. {c.level}</span><span class="rosterrow__lvsep">&nbsp;·&nbsp;</span>{/if}{c.class ?? 'No class'}{#if c.world !== DEFAULT_REGION}{` · ${regionLabel(c.world)}`}{/if}<span
          class="rosterrow__updated">&nbsp;· updated <span class="mono">{dayMonth(c.updated_at, kit.timeZone)}</span></span
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
      onkeydown={(event) => menuButtonKey(event, (at) => void showMenu(c.id, at))}><Icon name="more-vertical" /></button
    >
    {#if open}
      <!-- The pane's own Escape (close the member) must not fire too. -->
      <div
        class="rosterrow__menu"
        id="{uid}-menu"
        role="menu"
        aria-label={c.ign}
        tabindex="-1"
        data-fid="roster-menu"
        bind:this={menu}
        onkeydown={(event) => {
          if (event.key === 'Escape') event.stopPropagation();
          menuKey(event, menu, hideMenu);
        }}
      >
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
        {#if member}
          <!-- No Delete for members: the board says so where Delete would be. -->
          {#if locked}
            <span class="rosterrow__why" id="{uid}-why" data-fid="roster-menu-why">{c.ign} is your main. Make another character main first.</span>
          {:else}
            <span class="rosterrow__sep" role="separator"></span>
            <span class="rosterrow__why rosterrow__why--flush">Only an admin deletes a character. Retire keeps it for history.</span>
          {/if}
        {:else}
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
        {/if}
      </div>
    {/if}
  </li>
{/snippet}

<div class="rosterpanel" class:rosterpanel--phone={phone} data-fid="roster-panel" bind:this={panel}>
  {#if failed && !rows}
    <div class="rosterpanel__state" data-fid="roster-error">
      <LoadError thing="characters" reason={failed} level={3} {onretry} />
    </div>
  {:else if !rows}
    <LoadingState text="Loading characters…" />
  {:else if rows.length === 0}
    <div class="rosterpanel__state" data-fid="roster-empty">
      <StateNote icon="crown" title="No characters yet" level={3}>
        {member ? 'Your first character becomes your main; add alts after it. The admins see the same list.' : `${memberName}'s first character becomes the main; add alts after it.`}
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
      <ul class="rosterrows" aria-label={member ? 'Your characters' : `${memberName}'s characters`}>
        {#each groups.active as c (c.id)}{@render row(c, true)}{/each}
      </ul>
    {/if}
    {#if groups.retired.length}
      <section class="rosterpanel__retired" data-fid="roster-retired" aria-labelledby="{uid}-retired">
        <h3 class="cap" id="{uid}-retired">Retired</h3>
        <ul class="rosterrows" aria-label={member ? 'Your retired characters' : `${memberName}'s retired characters`}>
          {#each groups.retired as c (c.id)}{@render row(c, false)}{/each}
        </ul>
      </section>
    {/if}
  {/if}
  {#if rows && (rows.length || !member)}
    <p class="rosterpanel__note" data-fid="roster-note">
      {#if member}The same roster the admins see. Kept when you leave the server; only an admin deletes a character. Your changes show in History as yours. Characters are never
        sent to the chatbot.{:else}Kept when {memberName} leaves the server; only an admin deletes them. Characters are never sent to the chatbot.{/if}
    </p>
  {/if}
</div>
{#if !layer}{@render dialogs()}{/if}

{#snippet dialogs()}
  <!-- Mounted while in use: from the first open until it has closed (focus restored). -->
  {#if editorMounted}
    <CharacterDialog
      bind:open={dialogOpen}
      {kit}
      character={editing}
      rows={rows ?? []}
      sheet={phone}
      {fids}
      onsaved={(saved, message, linkable) => {
        toaster.show({ message, tone: 'ok' });
        void kit.api.refresh().then(() => focusRow(saved.id, 'edit'));
        if (!editing) kit.onadded?.({ row: saved, note: '', linkable });
      }}
      ondelete={kit.api.remove
        ? (c) => {
            dialogOpen = false;
            askDelete(c);
          }
        : undefined}
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
