<!--
  Add or edit one character (boards B_RosterEdit, B_PhoneRosterEdit): IGN,
  Region, Class, Level, Role (Main / Alt) and, when editing, Retired. Checks
  that need no server run first; the server's refusals (IGN taken in the
  region, a level out of range, the main rule, a stale version) land under
  their field and in one summary at the top. Input is kept on every failure.
  A bottom sheet in the phone frame, a dialog elsewhere.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { CharacterRow } from '@kanade/api-types';
  import { Icon, Modal, Select } from '@kanade/ui';
  import '@kanade/ui/styles/core/select.scss';
  import '@kanade/ui/styles/screens/settings.scss';
  import { send } from '../resource.svelte';
  import {
    checkForm,
    classOptions,
    dayMonth,
    DEFAULT_REGION,
    FIELD_LABELS,
    invalidField,
    mainOf,
    parseLevel,
    REGIONS,
    regionLabel,
    takenKey,
    takenText,
    type Field,
    type FieldErrors,
  } from './characters';

  let {
    open = $bindable(false),
    memberId,
    memberName,
    character,
    rows,
    sheet,
    timeZone,
    updatedBy,
    onsaved,
    ondelete,
    onrefresh,
    onclosed,
  }: {
    open: boolean;
    memberId: string;
    memberName: string;
    /** The character being edited; null adds one. */
    character: CharacterRow | null;
    /** The member's characters as listed (for the role hint and its rule). */
    rows: CharacterRow[];
    /** The phone frame: a bottom sheet with 44 px targets. */
    sheet: boolean;
    timeZone: string;
    updatedBy: (row: CharacterRow) => string;
    onsaved: (row: CharacterRow, message: string) => void;
    ondelete: (row: CharacterRow) => void;
    /** Re-read the list (after a stale or missing answer); resolves to the fresh rows. */
    onrefresh: () => Promise<CharacterRow[]>;
    /** Closed and focus given back: the caller may unmount it. */
    onclosed: () => void;
  } = $props();

  const uid = $props.id();
  let ign = $state('');
  let world = $state(DEFAULT_REGION);
  let klass = $state('');
  let level = $state('');
  let main = $state(false);
  let retired = $state(false);
  let version = $state(1);
  let errors = $state<FieldErrors>({});
  let problem = $state('');
  let busy = $state(false);
  // IGNs the server refused as taken, per region: kept while the IGN is unchanged.
  let taken = $state<ReadonlySet<string>>(new Set());
  let summary = $state<HTMLElement>();

  const others = $derived(rows.filter((c) => c.id !== character?.id));
  const otherActive = $derived(others.some((c) => c.retired_at === null));
  const currentMain = $derived(mainOf(others));
  // The first active character is the main, and the main stays while others are active.
  const firstActive = $derived(!otherActive);
  const keepsMain = $derived(character?.role === 'main' && character.retired_at === null && otherActive);

  // A fresh form each time it opens.
  $effect(() => {
    if (!open) return;
    ign = character?.ign ?? '';
    world = character?.world ?? DEFAULT_REGION;
    klass = character?.class ?? '';
    level = character?.level == null ? '' : String(character.level);
    main = character ? character.role === 'main' : firstActive;
    retired = character ? character.retired_at !== null : false;
    version = character?.version ?? 1;
    errors = {};
    problem = '';
    taken = new Set();
  });

  const roleHint = $derived.by(() => {
    if (keepsMain) return `${character!.ign} is ${memberName}'s main. Make another character main to move the crown.`;
    if (firstActive) return `${memberName}'s first character becomes the main.`;
    if (!currentMain) return '';
    return character ? `${memberName}'s main is ${currentMain.ign}. Choosing Main moves the crown here.` : `${memberName}'s main stays ${currentMain.ign}. Choose Main to move the crown here.`;
  });

  const listed = $derived((Object.keys(FIELD_LABELS) as Field[]).filter((f) => errors[f]));
  // One field wrong speaks for itself under the field; two or more, or a refusal of the whole save, get the summary.
  const showSummary = $derived(listed.length > 1 || Boolean(problem));

  function clear(field: Field) {
    if (errors[field]) errors = { ...errors, [field]: undefined };
  }

  const control = (field: Field) => document.querySelector<HTMLElement>(`[data-field="${uid}-${field}"] :is(input, select, button:not(:disabled))`);

  /** After a failed save: focus the summary, or the one field that needs fixing. */
  async function report(message = '') {
    problem = message;
    await tick();
    if (showSummary) summary?.focus({ preventScroll: false });
    else if (listed[0]) control(listed[0])?.focus();
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const local = checkForm({ ign, world, class: klass, level }, taken);
    errors = local;
    if (Object.keys(local).length) return report();
    const parsed = parseLevel(level) as number | null;
    const trimmed = ign.trim();
    busy = true;
    let result;
    if (character) {
      const patch: Record<string, unknown> = { version };
      if (trimmed !== character.ign) patch.ign = trimmed;
      if (world !== character.world) patch.world = world;
      if (klass !== (character.class ?? '')) patch.class = klass || null;
      if (parsed !== character.level) patch.level = parsed;
      if (main !== (character.role === 'main')) patch.role = main ? 'main' : 'alt';
      if (retired !== (character.retired_at !== null)) patch.retired = retired;
      result = await send((c) => c.patch<CharacterRow>(`/api/admin/characters/${encodeURIComponent(character!.id)}`, patch));
    } else {
      const body = { ign: trimmed, world, class: klass, level: parsed, role: main ? 'main' : 'alt' };
      result = await send((c) => c.post<CharacterRow>(`/api/admin/members/${encodeURIComponent(memberId)}/characters`, body));
    }
    busy = false;
    if (result.ok) {
      const row = result.value;
      onsaved(row, character ? `Saved ${row.ign}.` : `Added ${row.ign}${row.role === 'main' ? `, ${memberName}'s main` : ''}.`);
      open = false;
      return;
    }
    if (result.code === 'ign_taken') {
      taken = new Set([...taken, takenKey(trimmed, world)]);
      errors = { ign: takenText(trimmed, world) };
      return report();
    }
    if (result.code === 'invalid') {
      const field = invalidField(result.message);
      if (field) {
        errors = { [field.field]: field.text };
        return report();
      }
    }
    if (result.code === 'main_required') {
      errors = { role: `${memberName} keeps a main while other characters are active: make another character the main first.` };
      return report();
    }
    errors = {};
    if (result.code === 'stale' && character) {
      // Someone saved it meanwhile: take the new version and keep these edits.
      const fresh = (await onrefresh()).find((c) => c.id === character.id);
      if (fresh) {
        version = fresh.version;
        return report(`${character.ign} changed while you were editing it (now ${fresh.ign}${fresh.level ? `, Lv. ${fresh.level}` : ''}). Save again to replace that with your edits.`);
      }
    }
    if (result.status === 404) {
      void onrefresh();
      return report(`${character?.ign ?? 'This character'} is no longer on the roster.`);
    }
    return report(`Couldn't save ${trimmed || 'the character'}: ${result.message} Your edits are kept.`);
  }
</script>

{#snippet fieldError(field: Field, fid: string)}
  {#if errors[field]}<span class="rosterform__error" id="{uid}-{field}-error" data-fid={fid || undefined}><Icon name="alert-circle" /><span>{errors[field]}</span></span>{/if}
{/snippet}

{#snippet roleNote()}
  {#if errors.role}<span class="rosterform__error" id="{uid}-role-error"><Icon name="alert-circle" /><span>{errors.role}</span></span>
  {:else if roleHint}<span class="rosterform__hint" id="{uid}-role-hint">{roleHint}</span>{/if}
{/snippet}

<Modal
  bind:open
  title={character ? `Edit ${character.ign}` : 'Add character'}
  eyebrow={character ? `${memberName} · updated ${dayMonth(character.updated_at, timeZone)} by ${updatedBy(character)}` : memberName}
  narrow
  className="rosterdialog {sheet ? 'rosterdialog--sheet' : ''}"
  dismissible={!busy}
  fid="roster-dialog"
  fidParts={{ head: 'roster-dialog-head', foot: 'roster-dialog-foot' }}
  onclose={onclosed}
>
  <form class="rosterform" id="{uid}-form" novalidate onsubmit={save}>
    {#if showSummary}
      <div class="settings__box settings__box--risk rosterform__summary" role="alert" tabindex="-1" data-fid="roster-errors" bind:this={summary}>
        <Icon name="alert-triangle" />
        <span>
          {#if listed.length}<b>Fix {listed.length} field{listed.length === 1 ? '' : 's'} to save:</b>
            {#each listed as field, i (field)}{#if i},&#32;{/if}<button type="button" class="rosterform__jump" onclick={() => control(field)?.focus()}>{FIELD_LABELS[field]}</button>{/each}{/if}
          {#if problem}{listed.length ? ' ' : ''}{problem}{/if}
        </span>
      </div>
    {/if}
    <div class="rosterform__field" data-field="{uid}-ign" data-fid="roster-ign">
      <label class="cap" for="{uid}-ign">IGN</label>
      <input
        id="{uid}-ign"
        class="rosterform__input mono"
        bind:value={ign}
        oninput={() => clear('ign')}
        autocomplete="off"
        spellcheck="false"
        maxlength="12"
        aria-invalid={errors.ign ? 'true' : undefined}
        aria-describedby={errors.ign ? `${uid}-ign-error` : undefined}
      />
      {@render fieldError('ign', 'roster-ign-error')}
    </div>
    <div class="rosterform__pair">
      <div class="rosterform__field" data-field="{uid}-world" data-fid="roster-world">
        <span class="cap" id="{uid}-world-label">Region</span>
        <Select
          label="Region"
          value={world}
          options={REGIONS.some((r) => r.value === world) ? REGIONS : [{ value: world, label: regionLabel(world) }, ...REGIONS]}
          noun="regions"
          invalid={Boolean(errors.world)}
          error={errors.world ?? ''}
          onchange={(value) => {
            world = value;
            clear('world');
          }}
        />
        {#if world === DEFAULT_REGION && !sheet}<span class="rosterform__hint">The guild's default</span>{/if}
      </div>
      <div class="rosterform__field" data-field="{uid}-class" data-fid="roster-class">
        <span class="cap">Class</span>
        <Select
          label="Class"
          value={klass}
          options={classOptions(character?.class ?? null)}
          placeholder="Pick a class"
          noun="classes"
          invalid={Boolean(errors.class)}
          error={errors.class ?? ''}
          onchange={(value) => {
            klass = value;
            clear('class');
          }}
        />
      </div>
    </div>
    <div class="rosterform__levelrole">
      <div class="rosterform__field" data-field="{uid}-level" data-fid="roster-level">
        <label class="cap" for="{uid}-level">Level</label>
        <input
          id="{uid}-level"
          class="rosterform__input mono"
          bind:value={level}
          oninput={() => clear('level')}
          inputmode="numeric"
          autocomplete="off"
          aria-invalid={errors.level ? 'true' : undefined}
          aria-describedby={errors.level ? `${uid}-level-error` : undefined}
        />
        {#if !sheet}{@render fieldError('level', 'roster-level-error')}{/if}
      </div>
      <div class="rosterform__field" data-field="{uid}-role" data-fid="roster-role">
        <span class="cap" id="{uid}-role">Role</span>
        <div
          class="seg rosterform__role"
          role="group"
          aria-labelledby="{uid}-role"
          aria-describedby={errors.role ? `${uid}-role-error` : roleHint ? `${uid}-role-hint` : undefined}
        >
          <button
            type="button"
            class="seg__btn"
            id="{uid}-role-main"
            aria-pressed={main}
            disabled={retired}
            onclick={() => {
              main = true;
              clear('role');
            }}>Main</button
          >
          <button
            type="button"
            class="seg__btn"
            aria-pressed={!main}
            disabled={(firstActive && !retired) || keepsMain}
            onclick={() => {
              main = false;
              clear('role');
            }}>Alt</button
          >
        </div>
        {#if !sheet}{@render roleNote()}{/if}
      </div>
    </div>
    <!-- The phone sheet gives the level's error and the role's note the full width. -->
    {#if sheet}
      {@render fieldError('level', 'roster-level-error')}
      {@render roleNote()}
    {/if}
    {#if character}
      <div class="rosterform__switch" data-fid="roster-retired-switch">
        <span class="rosterform__switch-text">
          <b id="{uid}-retired">Retired</b>
          <span class="rosterform__hint" id="{uid}-retired-hint"
            >{keepsMain ? `The main stays while ${memberName} has other active characters.` : 'Kept for history; never offered when a run picks a character.'}</span
          >
        </span>
        <button
          type="button"
          role="switch"
          class="switch"
          aria-checked={retired}
          aria-labelledby="{uid}-retired"
          aria-describedby="{uid}-retired-hint"
          aria-disabled={keepsMain && !retired ? 'true' : undefined}
          onclick={() => {
            if (keepsMain && !retired) return;
            retired = !retired;
            if (retired) main = false;
          }}><span class="switch__knob" aria-hidden="true"></span></button
        >
      </div>
    {/if}
  </form>
  {#snippet footer(close)}
    {#if character}
      <button type="button" class="btn btn--danger" data-fid="roster-delete" disabled={busy} onclick={() => ondelete(character)}>Delete…</button>
      <span class="rosterform__grow"></span>
    {/if}
    <button type="button" class="btn" disabled={busy} onclick={close}>Cancel</button>
    <button type="submit" class="btn btn--primary btn--key" form="{uid}-form" data-fid="roster-save" disabled={busy}>{character ? 'Save' : 'Add character'}</button>
  {/snippet}
</Modal>
