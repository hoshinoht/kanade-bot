<!--
  Add or edit one character (boards B_RosterEdit, B_PhoneRosterEdit): IGN,
  Region, Class, Level, Role (Main / Alt) and, when editing, Retired. Checks
  that need no server run first; the server's refusals (IGN taken in the
  region, a level out of range, the main rule, a stale version) land under
  their field and in one summary at the top. Input is kept on every failure.
  A bottom sheet in the phone frame, a dialog elsewhere.

  Adding a MapleSEA character while NEXON is connected (module
  `character_data` with a key) asks no class or level: Look up (or Enter in
  the IGN) asks NEXON once and shows what it says, never stored (NEXON terms
  count copied values as NEXON data), and saving also asks the member to link
  it, from which class and level come once they confirm in Discord. Other
  regions, or no module or key, keep the manual fields; editing is unchanged.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { CharacterCreated, CharacterLookup, CharacterRow, SettingsActor } from '@kanade/api-types';
  import { Icon, LiveRegion, Modal, Select, StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/core/select.scss';
  import '@kanade/ui/styles/screens/settings.scss';
  import { send } from '../resource.svelte';
  import CharacterDataStrip from '../chardata/CharacterDataStrip.svelte';
  import { asOfDate, createdLinkNote, lookupLine, lookupMiss, type LookupMiss } from '../chardata/format';
  import { chardataRoutes, lookUp, lookupConnected } from '../chardata/routes.svelte';
  import {
    checkForm,
    classOptions,
    dayMonth,
    DEFAULT_REGION,
    FIELD_LABELS,
    IGN_ERROR,
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
    actor,
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
    /** A History actor's name (who asked for a link, who started a lookup). */
    actor: (who: SettingsActor) => string;
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
  // Adding: whether NEXON is connected (null until known, or when the read failed: manual fields).
  let connected = $state<boolean | null>(null);
  // The last lookup, for the IGN it was made with; cleared when the IGN or region changes.
  type Looked = { ign: string } & ({ state: 'busy' } | { state: 'found'; found: CharacterLookup } | { state: 'miss'; miss: LookupMiss; lead: string; text: string });
  let looked = $state<Looked | null>(null);
  let said = $state('');
  let opened = 0;
  const MISS_TAGS: Record<LookupMiss, { label: string; tone: 'neutral' | 'risk' | 'warn'; icon: 'alert-triangle' | 'clock' | 'plug' }> = {
    not_found: { label: 'not found', tone: 'risk', icon: 'alert-triangle' },
    held: { label: 'held', tone: 'warn', icon: 'clock' },
    not_connected: { label: 'not connected', tone: 'neutral', icon: 'plug' },
    wait: { label: 'wait', tone: 'warn', icon: 'clock' },
    failed: { label: 'failed', tone: 'risk', icon: 'alert-triangle' },
  };

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
    looked = null;
    said = '';
    connected = null;
    if (!character) {
      const mine = ++opened;
      // Untracked: the read may mark the module gone, which must not reset the form.
      void untrack(lookupConnected).then((value) => {
        if (mine === opened) connected = value;
      });
    }
  });

  // A MapleSEA add with NEXON connected: no manual class or level; Look up and the member's link instead.
  const nexon = $derived(!character && world === DEFAULT_REGION && connected === true);

  function forget() {
    if (looked) looked = null;
  }

  /** Look up (or Enter in the IGN): ask NEXON once; what it says is shown, never kept. */
  async function lookUpIgn() {
    const name = ign.trim();
    if (looked?.state === 'busy') return;
    if (!/^[A-Za-z0-9]{4,12}$/.test(name)) {
      errors = { ...errors, ign: IGN_ERROR };
      control('ign')?.focus();
      return;
    }
    looked = { ign: name, state: 'busy' };
    said = `Asking NEXON about ${name}…`;
    const result = await lookUp(name, world);
    // Typed on, or closed, meanwhile: this answer is for another name.
    if (!open || looked?.ign !== name || ign.trim() !== name) return;
    if (result.ok) {
      looked = { ign: name, state: 'found', found: result.value };
      said = `${result.value.name ?? name}: ${lookupLine(result.value)}. Shown only.`;
      return;
    }
    if (result.unbuilt) {
      // The module is off: the manual fields come back.
      chardataRoutes.gone = true;
      connected = false;
      looked = null;
      said = '';
      return;
    }
    const miss = lookupMiss(result.code, name, result.message);
    // No key after all: the manual fields come back, with the reason above them.
    if (miss.miss === 'not_connected') connected = false;
    looked = { ign: name, state: 'miss', ...miss };
    said = `${miss.lead} ${miss.text}`;
  }

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
    const local = checkForm({ ign, world, class: klass, level }, taken, !nexon);
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
      // Shown-only NEXON values are never sent: with NEXON connected a MapleSEA add has no class or level.
      const body = nexon ? { ign: trimmed, world, role: main ? 'main' : 'alt' } : { ign: trimmed, world, class: klass, level: parsed, role: main ? 'main' : 'alt' };
      const created = await send((c) => c.post<CharacterCreated>(`/api/admin/members/${encodeURIComponent(memberId)}/characters`, body));
      if (created.ok) {
        busy = false;
        const { link, ...row } = created.value;
        onsaved(row, `Added ${row.ign}${row.role === 'main' ? `, ${memberName}'s main` : ''}.${createdLinkNote(link, memberName)}`);
        open = false;
        return;
      }
      result = created;
    }
    busy = false;
    if (result.ok) {
      onsaved(result.value, `Saved ${result.value.ign}.`);
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

{#snippet ignInput()}
  <input
    id="{uid}-ign"
    class="rosterform__input mono"
    bind:value={ign}
    oninput={() => {
      clear('ign');
      forget();
    }}
    onkeydown={(event) => {
      // With NEXON connected, Enter in the IGN looks it up; the Add button saves.
      if (nexon && event.key === 'Enter' && !event.isComposing) {
        event.preventDefault();
        void lookUpIgn();
      }
    }}
    autocomplete="off"
    spellcheck="false"
    maxlength="12"
    aria-invalid={errors.ign ? 'true' : undefined}
    aria-describedby={[errors.ign ? `${uid}-ign-error` : '', looked && looked.state !== 'busy' ? `${uid}-lookup` : ''].filter(Boolean).join(' ') || undefined}
  />
{/snippet}

{#snippet regionField()}
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
        forget();
      }}
    />
    {#if world === DEFAULT_REGION && !sheet}<span class="rosterform__hint">The guild's default</span>{/if}
  </div>
{/snippet}

{#snippet roleField()}
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
{/snippet}

<!-- What NEXON said (shown only, never kept), or why it said nothing. -->
{#snippet lookupCard(shown: Looked)}
  <section class="rosterlookup" id="{uid}-lookup" aria-labelledby="{uid}-lookup-h" data-fid="roster-lookup">
    <div class="rosterlookup__head">
      <h3 class="cap" id="{uid}-lookup-h">NEXON lookup</h3>
      {#if shown.state === 'found'}
        <StatusChip tone="ok" data-fid="roster-lookup-state"><Icon name="check" />found</StatusChip>
      {:else if shown.state === 'miss'}
        {@const tag = MISS_TAGS[shown.miss]}
        <StatusChip tone={tag.tone} data-fid="roster-lookup-state"><Icon name={tag.icon} />{tag.label}</StatusChip>
      {/if}
    </div>
    {#if shown.state === 'found'}
      <span class="rosterlookup__value" data-fid="roster-lookup-values">{lookupLine(shown.found) || 'No class or level from NEXON'}</span>
      <span class="rosterlookup__sub">{shown.found.name ?? shown.ign} · as of {asOfDate(shown.found.as_of)} (SGT)</span>
      <span class="rosterlookup__sub">{shown.found.attribution}</span>
      <span class="rosterlookup__sub"><b>Shown only; saved after the member confirms in Discord.</b></span>
    {:else if shown.state === 'miss'}
      <span class="rosterlookup__value">{shown.lead}</span>
      <span class="rosterlookup__sub">{shown.text}</span>
    {/if}
  </section>
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
      {#if nexon}
        <div class="rosterform__ignrow">
          {@render ignInput()}
          <button type="button" class="btn" data-fid="roster-lookup-btn" disabled={looked?.state === 'busy' || busy} onclick={() => void lookUpIgn()}
            ><Icon name="search" />{looked?.state === 'busy' ? 'Looking up…' : 'Look up'}</button
          >
        </div>
      {:else}
        {@render ignInput()}
      {/if}
      {@render fieldError('ign', 'roster-ign-error')}
    </div>
    {#if looked && looked.state !== 'busy'}{@render lookupCard(looked)}{/if}
    {#if nexon}
      <div class="rosterform__pair">
        {@render regionField()}
        {@render roleField()}
      </div>
      {#if sheet}{@render roleNote()}{/if}
      <p class="rosterform__note" id="{uid}-link-note" data-fid="roster-link-note">
        <Icon name="link" /><span
          >Adding asks {memberName} to link {ign.trim() || 'it'}: nothing from NEXON is kept until {memberName} confirms with
          <span class="mono">/character link</span> in Discord, and class and level then come from that link. Kanade doesn’t message {memberName}; tell them.</span
        >
      </p>
    {:else}
      <div class="rosterform__pair">
        {@render regionField()}
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
        {@render roleField()}
      </div>
      <!-- The phone sheet gives the level's error and the role's note the full width. -->
      {#if sheet}
        {@render fieldError('level', 'roster-level-error')}
        {@render roleNote()}
      {/if}
    {/if}
    <LiveRegion message={said} />
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
      <!-- Module `character_data`: NEXON data and estimates (draws nothing while the module is off). -->
      <CharacterDataStrip {character} {memberName} {sheet} {timeZone} updatedBy={updatedBy(character)} {actor} />
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
