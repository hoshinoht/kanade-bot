<!--
  The Fixed editor's Characters tab (modules `roster` and `character_data`;
  B_FixedReady, B_FixedReadyEnd, B_FixedReadyPick, B_FixedReadyStates*,
  B_PhoneFixedReady*): who each party member brings to this weekly timing and
  that character's readiness, as the run sheet draws it. A member brings their
  main, else their only character; a pick here overrides that for this timing
  only, saves at once (never through the form; Undo in the notice) and runs
  made from the timing read it live unless they have their own pick. Readiness
  follows the form as it stands (a preview while bosses or party differ from
  what is saved) and is shown, never judged: it never blocks a pick or a save.
  Picks open once the timing, and the member in its party, are saved.
-->
<script lang="ts">
  import type { BossRow, CharacterChoice, ReadinessSlot, RunCharacterSlot, TimingCharacters } from '@kanade/api-types';
  import { dayMonth, Icon } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-pick.scss';
  import '@kanade/ui/styles/screens/settings.scss';
  import { tick, untrack } from 'svelte';
  import { scouterAt } from '../chardata/format';
  import ReadinessNote from '../chardata/ReadinessNote.svelte';
  import ReadinessValues from '../chardata/ReadinessValues.svelte';
  import { estimateEnds, fixedReadyRoutes, TimingReadinessView, type TimingTarget } from '../chardata/routes.svelte';
  import { actorName } from '../history/describe';
  import { live } from '../resource.svelte';
  import CharacterPicker from '../sheet/CharacterPicker.svelte';
  import { CharacterSlots, shownCharacter } from '../sheet/characters.svelte';

  let {
    fixedId,
    savedParty,
    party,
    tokens,
    formMatches,
    catalog,
    wide,
    timeZone,
    title,
    eyebrow,
    names,
    known,
  }: {
    /** The saved timing; null while it is not added yet. */
    fixedId: string | null;
    /** The saved party (member ids); empty for a new timing. */
    savedParty: readonly string[];
    /** The form's party, in its order. */
    party: readonly string[];
    /** The form's boss tokens (picked and typed). */
    tokens: readonly string[];
    /** The form's bosses and party are the saved ones: read the timing, not a preview. */
    formMatches: boolean;
    catalog: readonly BossRow[];
    wide: boolean;
    timeZone: string;
    /** "Tuesday 22:00 — HCarling + HStar". */
    title: string;
    /** The phone picker's line over its title: "HCarling + HStar · Tue 22:00 weekly". */
    eyebrow: string;
    /** A member as the editor names them ("Ren (1)"). */
    names: (id: string) => string;
    /** The id is a member (an admin's Discord id names them). */
    known: (id: string) => boolean;
  } = $props();

  // Only the saved party has slots (and picks); members added in the form are drawn from the preview.
  const chars = $derived(fixedId ? new CharacterSlots<TimingCharacters>('fixed', fixedId, fixedReadyRoutes) : null);
  $effect(() => void chars?.load());
  $effect(() => chars?.follow());

  const ready = new TimingReadinessView();
  // A preview waits for the form to rest; the key keeps an equal form from asking again.
  const PREVIEW_MS = 300;
  const target = $derived<TimingTarget>(
    fixedId && formMatches ? { fixedId } : { fixed_run_id: fixedId, bosses: [...tokens], participants: [...party] },
  );
  const targetKey = $derived(JSON.stringify(target));
  $effect(() => {
    void targetKey;
    // A finished scouter lookup replaces CP: read again.
    void estimateEnds.count;
    const next = untrack(() => target);
    if ('fixedId' in next) {
      void ready.load(next);
      return;
    }
    const timer = setTimeout(() => void ready.load(next), PREVIEW_MS);
    return () => clearTimeout(timer);
  });
  // Another admin's pick or a change to a character arrives as a schedule hint.
  $effect(() => live.subscribe(['schedule'], () => void ready.load()));

  const notSaved = $derived(fixedId ? party.filter((id) => !savedParty.includes(id)) : []);
  const scouterShown = $derived(Boolean(ready.data?.slots.some((s) => scouterAt(s) !== null)));
  const firstRead = $derived(ready.loading && !ready.data);
  const error = $derived(ready.error || chars?.error || '');
  // The timing's bosses with their entry levels, as the readiness judged them.
  const entries = $derived(
    (ready.data?.slots.find((s) => s.bosses.length)?.bosses ?? []).map((b) => ({ label: bossLabel(b.boss, b.difficulty), entry: b.entry_level })),
  );

  function bossLabel(key: string, difficulty: string): string {
    return catalog.find((b) => b.key === key)?.difficulties.find((d) => d.letter === difficulty)?.token ?? key;
  }

  type Row = {
    id: string;
    name: string;
    /** In the form's party but not the saved one. */
    unsaved: boolean;
    slot: RunCharacterSlot | null;
    ready: ReadinessSlot | null;
    character: CharacterChoice | null;
    basis: RunCharacterSlot['basis'];
    /** Active characters; null until read. */
    count: number | null;
    main: CharacterChoice | null;
  };

  const rows = $derived<Row[]>(
    party.map((id) => {
      const slot = chars?.slot(id) ?? null;
      const r = ready.data?.slots.find((s) => s.member_id === id) ?? null;
      const character = slot ? shownCharacter(slot) : (r?.character ?? null);
      return {
        id,
        name: names(id),
        unsaved: Boolean(fixedId) && !savedParty.includes(id),
        slot,
        ready: r,
        character,
        basis: slot?.basis ?? r?.basis ?? null,
        count: slot?.candidates.length ?? r?.candidates ?? null,
        main: slot?.candidates.find((c) => c.role === 'main') ?? (r?.basis === 'main' ? r.character : null),
      };
    }),
  );

  /** Why a row's pick is closed, or '' while it is open. */
  function locked(row: Row): string {
    if (!fixedId) return 'Picks open once the timing is added.';
    if (row.unsaved) return `Save the party to pick for ${row.name}.`;
    return '';
  }

  /** What a member brings with nothing picked: the main, else their only character. */
  function fallback(slot: RunCharacterSlot): CharacterChoice | null {
    return slot.candidates.find((c) => c.role === 'main') ?? (slot.candidates.length === 1 ? slot.candidates[0]! : null);
  }

  const BASIS_WORDS: Record<NonNullable<RunCharacterSlot['basis']>, string> = {
    run_pick: 'picked for this run',
    timing_pick: 'picked for this timing',
    main: 'main',
    only: 'only character',
  };

  // The laptop's popover (one member's), the phone's bottom sheet.
  let pickFor = $state<string | null>(null);
  let sheetFor = $state<string | null>(null);
  let sheetOpen = $state(false);
  let busy = $state(false);
  let panel = $state<HTMLElement>();
  const trigger = (member: string) => panel?.querySelector<HTMLElement>(`[data-char-for="${CSS.escape(member)}"]`) ?? null;
  const sheetRow = $derived(rows.find((r) => r.id === sheetFor) ?? null);

  type Notice = { member: string; tone: 'ok' | 'warn' | 'risk'; lead: string; ign?: string; tail?: string; undo?: () => void };
  let notice = $state<Notice | null>(null);
  const announcement = $derived(notice ? `${notice.lead}${notice.ign ?? ''}${notice.tail ?? ''}` : '');

  function open(row: Row) {
    if (!row.slot || locked(row)) return;
    if (wide) pickFor = pickFor === row.id ? null : row.id;
    else {
      sheetFor = row.id;
      sheetOpen = true;
    }
  }

  async function closePick(member: string, refocus: boolean) {
    if (pickFor === member) pickFor = null;
    if (!refocus) return;
    await tick();
    trigger(member)?.focus({ preventScroll: true });
  }

  /** A choice: the member's default clears the pick; what is picked already changes nothing. */
  function choose(row: Row, characterId: string | null) {
    const slot = chars?.slot(row.id);
    if (!slot) return;
    const next = characterId !== null && characterId === fallback(slot)?.id ? null : characterId;
    if (next === slot.picked) {
      pickFor = null;
      sheetOpen = false;
      return;
    }
    void save(row, next, slot.picked, true);
  }

  async function save(row: Row, characterId: string | null, before: string | null, undoable: boolean) {
    if (!chars || busy) return;
    busy = true;
    const result = await chars.pick(row.id, characterId).finally(() => (busy = false));
    pickFor = null;
    sheetOpen = false;
    if (result.ok) {
      const shown = shownCharacter(result.value);
      const undo = undoable && before !== characterId ? () => void save(row, before, characterId, false) : undefined;
      if (characterId && shown) notice = { member: row.id, tone: 'ok', lead: `${row.name} brings `, ign: shown.ign, tail: ' to this timing.', undo };
      else if (shown) notice = { member: row.id, tone: 'ok', lead: `${row.name} is back on `, ign: shown.ign, tail: ` (${BASIS_WORDS[result.value.basis ?? 'main']}).`, undo };
      else notice = { member: row.id, tone: 'ok', lead: `Cleared ${row.name}'s pick.`, undo };
      void ready.load();
    } else if (result.code === 'stale') {
      // The slot was read again: say whose pick it shows now.
      const now = chars.slot(row.id);
      const shown = now ? shownCharacter(now) : null;
      const by = now?.picked_by ? actorName(now.picked_by, names, known) : null;
      const lead = by ? `${by} changed ${row.name}'s pick a moment ago` : `${row.name}'s pick changed a moment ago`;
      const basis = now?.basis ? ` (${BASIS_WORDS[now.basis]})` : '';
      notice = shown
        ? { member: row.id, tone: 'warn', lead: `${lead}${wide ? ", so yours wasn't saved" : ''}. Showing ${by ? 'theirs' : 'it now'}: `, ign: shown.ign, tail: `${basis}.${wide ? ' Pick again to change it.' : ''}` }
        : { member: row.id, tone: 'warn', lead: `${lead}, so yours wasn't saved. Nothing is brought now.` };
      void ready.load();
    } else if (!result.unbuilt) {
      notice = { member: row.id, tone: 'risk', lead: `Couldn't set ${row.name}'s character: ${result.message}` };
    }
    await tick();
    trigger(row.id)?.focus({ preventScroll: true });
  }

  /** "the default", "picked 28 Sep by Asahi", "under HStar's Lv. 280". */
  function meta(slot: RunCharacterSlot, c: CharacterChoice): string[] {
    const out: string[] = [];
    if (c.id === fallback(slot)?.id) out.push('the default');
    if (c.id === slot.picked && slot.picked_at) out.push(`picked ${dayMonth(slot.picked_at, timeZone)}${slot.picked_by ? ` by ${actorName(slot.picked_by, names, known)}` : ''}`);
    const above = under(c);
    if (above) out.push(`under ${above.label}’s Lv. ${above.entry}`);
    return out;
  }

  /** The picker's note (B_FixedReadyPick, B_FixedReadyStates "no main"). */
  function pickWords(row: Row, def: CharacterChoice | null): string {
    if (!def) return `${row.name} has no main, so nothing is brought until you pick. This pick is for this timing only.`;
    const back = def.role === 'main' ? ` ${wide ? 'Choosing ' : ''}${def.ign} goes back to the main.` : '';
    return `Saves at once. Runs made from this timing use it unless the run has its own pick.${back}`;
  }

  /** The highest entry level above the character's level: tinted and named, still offered. */
  function under(c: CharacterChoice): { label: string; entry: number } | null {
    if (c.level === null) return null;
    let worst: { label: string; entry: number } | null = null;
    for (const { label, entry } of entries) if (entry !== null && entry > c.level && (!worst || entry > worst.entry)) worst = { label, entry };
    return worst;
  }
</script>

{#snippet basisTags(row: Row)}
  <span class="fixedready__tags">
    {#if row.unsaved}<span class="fixedready__basis fixedready__basis--new">not saved yet</span>{/if}
    {#if row.character && row.basis === 'timing_pick'}
      <span class="fixedready__basis fixedready__basis--picked" data-fid="fixed-ready-basis"><Icon name="pin" />picked for this timing</span>
      {#if row.main && row.main.id !== row.character.id}<span>main is {row.main.ign}</span>{/if}
    {:else if row.character && row.basis === 'main'}
      <span class="fixedready__basis fixedready__basis--main" data-fid="fixed-ready-basis"><Icon name="crown" />main</span>
      {#if row.count && row.count > 1}<span>{row.count} characters</span>{/if}
    {:else if row.character && row.basis === 'only'}
      <span class="fixedready__basis fixedready__basis--only" data-fid="fixed-ready-basis">only character</span>
    {:else if row.count === 0}
      <span class="fixedready__basis fixedready__basis--none" data-fid="fixed-ready-basis">no characters</span>
    {:else if row.count}
      <span class="fixedready__basis fixedready__basis--none" data-fid="fixed-ready-basis">no main</span>
      <span>{row.count} characters</span>
    {/if}
  </span>
{/snippet}

{#snippet control(row: Row)}
  {@const why = locked(row)}
  {@const expanded = pickFor === row.id || (sheetOpen && sheetFor === row.id)}
  {#if row.character}
    <button
      type="button"
      class="slotchar__btn slotchar__chip"
      data-fid="fixed-ready-char"
      data-char-for={row.id}
      aria-haspopup="dialog"
      aria-expanded={why ? undefined : expanded}
      aria-disabled={why || !row.slot ? 'true' : undefined}
      aria-label={why ? `${row.name}'s character: ${row.character.ign}. ${why}` : `${row.name}'s character for this timing: ${row.character.ign}, change`}
      onclick={() => open(row)}
      ><span class="slotchar__ign">{row.character.ign}</span>{#if wide && row.character.class}<span class="slotchar__meta">{row.character.class}</span>{/if}{#if !why}<Icon
          name="chevron-down"
        />{/if}</button
    >
  {:else if row.count}
    <button
      type="button"
      class="slotchar__btn slotchar__pick"
      data-fid="fixed-ready-pick"
      data-char-for={row.id}
      aria-haspopup="dialog"
      aria-expanded={why ? undefined : expanded}
      aria-disabled={why || !row.slot ? 'true' : undefined}
      aria-label={why ? `${row.name} has no main. ${why}` : `Pick ${row.name}'s character for this timing: ${row.count} to choose from`}
      onclick={() => open(row)}>Pick character<span class="slotchar__count mono">{row.count}</span>{#if !why}<Icon name="chevron-down" />{/if}</button
    >
  {/if}
{/snippet}

{#snippet pickNote(row: Row, slot: RunCharacterSlot)}
  {@const def = fallback(slot)}
  {#if wide}<span class="charpick__sep" aria-hidden="true"></span>{/if}
  <p class="charpick__note" data-fid="fixed-pick-note">{pickWords(row, def)}</p>
{/snippet}

<div class="fixedready" class:fixedready--wide={wide} bind:this={panel}>
  <ReadinessNote
    lead={wide ? ' · never blocks a pick or a save' : ' · never blocks'}
    attribution={ready.data?.nexon_data ? ready.data.attribution : null}
    scouter={scouterShown}
    fid="fixed-ready"
  />
  {#if !fixedId}
    <p class="settings__box fixedready__box" data-fid="fixed-ready-new">
      <Icon name="info" /><span
        >{wide ? 'Readiness follows the form as you pick bosses and party.' : 'Readiness follows the form.'} <b>Picks open once the timing is added</b>; until then each
        member brings their default.</span
      >
    </p>
  {:else if notSaved.length}
    {@const list = notSaved.map(names).join(', ')}
    <p class="settings__box settings__box--warn fixedready__box">
      <Icon name="alert-triangle" /><span
        >The party changed and isn’t saved: {list} {notSaved.length === 1 ? 'is' : 'are'} drawn from the form. Save to pick a character for {list}.</span
      >
    </p>
  {/if}
  {#if error}
    <div class="settings__box settings__box--risk fixedready__box" data-fid="fixed-ready-error" role="alert">
      <Icon name="alert-triangle" /><span class="fixedready__grow">Readiness couldn’t load{wide ? ` (${error.replace(/\.$/, '')})` : ''}. Picks and the timing still save.</span>
      <button
        type="button"
        class="btn fixedready__retry"
        onclick={() => {
          void chars?.load();
          void ready.load();
        }}><Icon name="refresh-cw" />Try again</button
      >
    </div>
  {/if}
  {#if firstRead}<p class="fixedready__hint" role="status">Reading levels and CP…</p>{/if}

  {#if party.length === 0}
    <p class="fixedready__hint">No one in the party yet: pick members on the Timing tab.</p>
  {:else}
    <ul class="fixedready__rows" aria-label="Party characters">
      {#each rows as row (row.id)}
        <li class="fixedready__row" class:fixedready__row--new={row.unsaved} data-fid="fixed-ready-slot">
          <div class="fixedready__head">
            <span class="fixedready__who"><b class="fixedready__name">{row.name}</b>{@render basisTags(row)}</span>
            <span class="slotchar fixedready__control">
              {@render control(row)}
              {#if wide && pickFor === row.id && row.slot}
                {@const slot = row.slot}
                <CharacterPicker
                  variant="popover"
                  what="timing"
                  fid="fixed-pick-menu"
                  optionFid="fixed-pick-option"
                  tagEveryOption
                  {slot}
                  name={row.name}
                  {title}
                  {busy}
                  meta={(c) => meta(slot, c)}
                  under={(c) => under(c) !== null}
                  onpick={(id) => choose(row, id)}
                  onclose={(refocus) => void closePick(row.id, refocus)}
                >
                  {#snippet note()}{@render pickNote(row, slot)}{/snippet}
                </CharacterPicker>
              {/if}
            </span>
          </div>
          <!-- Values only for the character the row shows: right after a pick they wait for the next read. -->
          {#if row.character && row.ready?.character?.id === row.character.id && !error}
            <ReadinessValues
              slot={row.ready}
              character={row.ready.character}
              {timeZone}
              {bossLabel}
              fid="fixed-ready-values"
              bossesFid="fixed-ready-bosses"
              row={wide}
            />
          {:else if row.character && ready.loading && !error}
            <span class="fixedready__skel fixedready__skel--long"></span><span class="fixedready__skel fixedready__skel--short"></span>
          {/if}
          {#if row.count === 0}
            <span class="fixedready__hint"
              >No characters yet · <a href="/members?open={encodeURIComponent(row.id)}" aria-label="{row.name} has no characters yet: add them on Members"
                >add on Members</a
              ></span
            >
          {:else if row.count && !row.character && !error}
            <span class="fixedready__hint"
              >Readiness shows once a character is picked. <a href="/members?open={encodeURIComponent(row.id)}">Set a main on Members</a> to make one the default
              everywhere.</span
            >
          {/if}
        </li>
        {#if notice?.member === row.id}
          <li class="fixedready__notice">
            <div class="fixedready__status fixedready__status--{notice.tone}" data-fid="fixed-pick-status">
              <Icon name={notice.tone === 'ok' ? 'check' : 'alert-triangle'} /><span class="fixedready__grow"
                >{notice.lead}{#if notice.ign}<b>{notice.ign}</b>{/if}{notice.tail ?? ''}</span
              >{#if notice.undo}<button type="button" class="btn" disabled={busy} onclick={notice.undo}>Undo</button>{/if}
            </div>
          </li>
        {/if}
      {/each}
    </ul>
  {/if}

  <p class="fixedready__hint" data-fid="fixed-ready-default">
    {wide
      ? 'Each member brings their main, else their only character. A pick here overrides that for this timing only and saves at once; runs made from this timing use it unless the run has its own pick.'
      : 'Each member brings their main, else their only character. A pick here saves at once; runs from this timing use it.'}
  </p>
  <p class="fixedready__hint" data-fid="fixed-ready-rules">
    {#snippet levels()}{#each entries as e, i (e.label)}{i ? ', ' : ''}{e.label}
        {#if e.entry !== null}<span class="mono">Lv. {e.entry}</span>{:else}no entry level{/if}{/each}{/snippet}
    {#if wide}
      Per boss of the timing: level against its entry level{#if entries.length}&nbsp;({@render levels()}){/if}. CP, or the estimate where a lookup is kept (it
      replaces CP), is shown, never judged. Fresh NEXON data first; else the roster level, marked manual.
    {:else}
      Level against each boss’s entry level{#if entries.length}: {@render levels()}{/if}. CP and estimates are shown, never judged.
    {/if}
  </p>
  <!-- Said once, and kept out of the visible notice so its Undo stays a plain button. -->
  <span class="vh" role="status" aria-live="polite" aria-atomic="true">{announcement}</span>
</div>

{#if !wide && sheetRow?.slot}
  {@const row = sheetRow}
  {@const slot = sheetRow.slot}
  <CharacterPicker
    variant="sheet"
    what="timing"
    fid="fixed-pick-menu"
    optionFid="fixed-pick-option"
    tagEveryOption
    bind:open={sheetOpen}
    {slot}
    name={row.name}
    {title}
    {eyebrow}
    {busy}
    meta={(c) => meta(slot, c)}
    under={(c) => under(c) !== null}
    onpick={(id) => choose(row, id)}
    returnFocus={() => trigger(row.id)}
  >
    {#snippet note()}{@render pickNote(row, slot)}{/snippet}
  </CharacterPicker>
{/if}
