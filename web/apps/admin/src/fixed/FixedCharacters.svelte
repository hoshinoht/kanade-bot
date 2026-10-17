<!--
  The Fixed editor's Characters tab (modules `roster` and `character_data`;
  B_FixedReady, B_FixedReadyEnd, B_FixedReadyPick, B_FixedReadyStates*,
  B_PhoneFixedReady*; per boss B_FixedBoss2*, B_FixedBoss4, B_PhoneFixedBoss*,
  B_BossPickStates): who each party member brings to this weekly timing and
  that character's readiness, as the run sheet draws it. A member brings their
  main, else their only character; a pick here overrides that for this timing
  only, saves at once (never through the form; Undo in the notice) and runs
  made from the timing read it live unless they have their own pick.
  Per boss: a member with more than one character can open "Per boss" and
  pick another character for a later boss (a boss with no pick of its own
  brings the first boss's, dashed "same as HCarling"); a member with any
  per-boss pick always shows the list, with "Same for every boss" to clear
  them in one write. Readiness follows the form as it stands (a preview while
  bosses or party differ from what is saved) and is shown, never judged: it
  never blocks a pick or a save. Picks open once the timing, and the member
  in its party, are saved; per boss once the form's bosses are the saved ones.
-->
<script lang="ts">
  import type { BossCharacterSlot, BossRow, CharacterChoice, ReadinessSlot, RunCharacterSlot, TimingCharacters } from '@kanade/api-types';
  import { dayMonth, Icon, StatusChip } from '@kanade/ui';
  import '@kanade/ui/styles/screens/character-pick.scss';
  import '@kanade/ui/styles/screens/settings.scss';
  import { tick, untrack } from 'svelte';
  import { levelChip, readinessLine, scouterAt } from '../chardata/format';
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
    (ready.data?.slots.find((s) => s.bosses.length)?.bosses ?? []).map((b) => ({ key: b.boss, label: bossLabel(b.boss, b.difficulty), entry: b.entry_level })),
  );
  const firstBoss = $derived(entries[0]?.label ?? tokens[0] ?? '');

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

  // Per boss: open for a member with any per-boss pick, or opened here; only while the form's bosses are the saved ones.
  let opened = $state<string[]>([]);
  const perBossReady = (row: Row) => Boolean(row.slot && formMatches && !locked(row) && row.slot.bosses.length > 1);
  const perBossOpen = (row: Row) => perBossReady(row) && (row.slot!.per_boss || opened.includes(row.id));

  // The laptop's popover (one cell's), the phone's bottom sheet; `i` is the boss (0: the member's own pick).
  let pickFor = $state<string | null>(null);
  let sheetFor = $state<{ member: string; i: number } | null>(null);
  let sheetOpen = $state(false);
  let busy = $state(false);
  let panel = $state<HTMLElement>();
  const keyOf = (member: string, i: number) => `${member}:${i}`;
  const trigger = (member: string, i = 0) => panel?.querySelector<HTMLElement>(`[data-char-for="${CSS.escape(keyOf(member, i))}"]`) ?? null;
  const sheetRow = $derived(rows.find((r) => r.id === sheetFor?.member) ?? null);

  type Notice = { member: string; tone: 'ok' | 'warn' | 'risk'; lead: string; ign?: string; tail?: string; undo?: () => void };
  let notice = $state<Notice | null>(null);
  const announcement = $derived(notice ? `${notice.lead}${notice.ign ?? ''}${notice.tail ?? ''}` : '');

  function open(row: Row, i = 0) {
    if (!row.slot || locked(row)) return;
    if (wide) pickFor = pickFor === keyOf(row.id, i) ? null : keyOf(row.id, i);
    else {
      sheetFor = { member: row.id, i };
      sheetOpen = true;
    }
  }

  async function closePick(member: string, i: number, refocus: boolean) {
    if (pickFor === keyOf(member, i)) pickFor = null;
    if (!refocus) return;
    await tick();
    trigger(member, i)?.focus({ preventScroll: true });
  }

  /**
   * A choice: the member's default clears the pick (for a later boss, the first boss's character
   * does); what is picked already changes nothing.
   */
  function choose(row: Row, i: number, characterId: string | null) {
    const slot = chars?.slot(row.id);
    if (!slot) return;
    const entry = i > 0 ? slot.bosses[i] : null;
    const base = entry ? slot.brings : (fallback(slot)?.id ?? null);
    const next = characterId !== null && characterId === base ? null : characterId;
    const picked = entry ? entry.picked : slot.picked;
    if (next === picked) {
      pickFor = null;
      sheetOpen = false;
      return;
    }
    void save(row, i, next, picked, true);
  }

  async function save(row: Row, i: number, characterId: string | null, before: string | null, undoable: boolean) {
    if (!chars || busy) return;
    const entry = i > 0 ? (chars.slot(row.id)?.bosses[i] ?? null) : null;
    busy = true;
    const result = await (entry ? chars.pickBoss(row.id, entry.key, characterId) : chars.pick(row.id, characterId)).finally(() => (busy = false));
    pickFor = null;
    sheetOpen = false;
    if (result.ok) {
      const undo = undoable && before !== characterId ? () => void save(row, i, before, characterId, false) : undefined;
      if (entry) {
        const now = result.value.bosses[i];
        const shown = result.value.candidates.find((c) => c.id === now?.brings) ?? null;
        notice = shown
          ? characterId
            ? { member: row.id, tone: 'ok', lead: `${row.name} brings `, ign: shown.ign, tail: ` to ${entry.boss} on this timing.`, undo }
            : { member: row.id, tone: 'ok', lead: `${row.name}'s ${entry.boss} is back on `, ign: shown.ign, tail: ` (same as ${firstBoss}).`, undo }
          : { member: row.id, tone: 'ok', lead: `Cleared ${row.name}'s ${entry.boss} pick.`, undo };
      } else {
        const shown = shownCharacter(result.value);
        if (characterId && shown) notice = { member: row.id, tone: 'ok', lead: `${row.name} brings `, ign: shown.ign, tail: ' to this timing.', undo };
        else if (shown) notice = { member: row.id, tone: 'ok', lead: `${row.name} is back on `, ign: shown.ign, tail: ` (${BASIS_WORDS[result.value.basis ?? 'main']}).`, undo };
        else notice = { member: row.id, tone: 'ok', lead: `Cleared ${row.name}'s pick.`, undo };
      }
      void ready.load();
    } else if (result.code === 'stale') {
      // The slot was read again: say whose pick it shows now.
      const now = chars.slot(row.id);
      const at = entry ? now?.bosses[i] : now;
      const shown = now ? (now.candidates.find((c) => c.id === at?.brings) ?? null) : null;
      const by = at?.picked_by ? actorName(at.picked_by, names, known) : null;
      const what = entry ? `${row.name}'s ${entry.boss} pick` : `${row.name}'s pick`;
      const lead = by ? `${by} changed ${what} a moment ago` : `${what.charAt(0).toUpperCase()}${what.slice(1)} changed a moment ago`;
      const basis = !entry && now?.basis ? ` (${BASIS_WORDS[now.basis]})` : '';
      notice = shown
        ? { member: row.id, tone: 'warn', lead: `${lead}${wide ? ", so yours wasn't saved" : ''}. Showing ${by ? 'theirs' : 'it now'}: `, ign: shown.ign, tail: `${basis}.${wide ? ' Pick again to change it.' : ''}` }
        : { member: row.id, tone: 'warn', lead: `${lead}, so yours wasn't saved. Nothing is brought now.` };
      void ready.load();
    } else if (!result.unbuilt) {
      notice = { member: row.id, tone: 'risk', lead: `Couldn't set ${row.name}'s character: ${result.message}` };
    }
    await tick();
    trigger(row.id, i)?.focus({ preventScroll: true });
  }

  /** "Same for every boss": one write; Undo puts each per-boss pick back. */
  async function same(row: Row) {
    const slot = chars?.slot(row.id);
    if (!chars || !slot || busy) return;
    const kept = slot.bosses.slice(1).filter((b) => b.picked);
    busy = true;
    const result = await chars.same(row.id).finally(() => (busy = false));
    if (result.ok) {
      const shown = shownCharacter(result.value);
      notice = {
        member: row.id,
        tone: 'ok',
        lead: shown ? `${row.name} brings ` : `${row.name}'s per-boss picks are cleared.`,
        ign: shown?.ign,
        tail: shown ? ' to every boss again.' : undefined,
        undo: kept.length ? () => void restore(row, kept) : undefined,
      };
      void ready.load();
    } else if (result.code === 'stale') {
      notice = { member: row.id, tone: 'warn', lead: `${row.name}'s per-boss picks changed a moment ago, so nothing was cleared. Showing them now.` };
      void ready.load();
    } else if (!result.unbuilt) {
      notice = { member: row.id, tone: 'risk', lead: `Couldn't set ${row.name}'s characters: ${result.message}` };
    }
    await tick();
    trigger(row.id, 0)?.focus({ preventScroll: true });
  }

  /** Undo of "Same for every boss": a failed restore stops there and keeps an Undo for it and the rest. */
  async function restore(row: Row, kept: BossCharacterSlot[]) {
    if (!chars) return;
    busy = true;
    let done = 0;
    try {
      for (const b of kept) {
        const back = await chars.pickBoss(row.id, b.key, b.picked);
        if (!back.ok) {
          const left = kept.slice(done);
          notice = {
            member: row.id,
            tone: 'risk',
            lead: `Put back ${done} of ${kept.length} of ${row.name}'s per-boss picks; ${b.boss} failed (${back.message}). Undo tries the ${left.length === 1 ? 'last one' : `other ${left.length}`} again.`,
            undo: () => void restore(row, left),
          };
          return;
        }
        done += 1;
      }
      notice = { member: row.id, tone: 'ok', lead: `${row.name}'s per-boss picks are back.` };
    } finally {
      busy = false;
      void ready.load();
    }
  }

  /** "the default", "same as HCarling", "picked 28 Sep by Asahi", "under HStar's Lv. 280". */
  function meta(slot: RunCharacterSlot, i: number, c: CharacterChoice): string[] {
    const out: string[] = [];
    const at = i > 0 ? slot.bosses[i] : slot;
    if (i === 0 && c.id === fallback(slot)?.id) out.push('the default');
    if (i > 0 && c.id === slot.brings) out.push(`same as ${firstBoss}`);
    if (at && c.id === at.picked && at.picked_at) out.push(`picked ${dayMonth(at.picked_at, timeZone)}${at.picked_by ? ` by ${actorName(at.picked_by, names, known)}` : ''}`);
    const above = under(c, i > 0 ? slot.bosses[i]?.key : undefined);
    if (above) out.push(`under ${above.label}’s Lv. ${above.entry}`);
    return out;
  }

  /** The picker's note (B_FixedReadyPick, B_FixedReadyStates "no main", B_FixedBoss2Pick; the phone's per boss B_PhoneFixedBossPick). */
  function pickWords(row: Row, slot: RunCharacterSlot, i: number): string {
    if (i > 0) {
      const back = shownCharacter(slot);
      const boss = slot.bosses[i]?.boss ?? 'this boss';
      const lead = wide ? `Runs made from this timing bring it to ${boss} unless the run has its own pick.` : `Runs from this timing bring it to ${boss}.`;
      return `Saves at once. ${lead}${back ? ` Choosing ${back.ign} goes back to ${firstBoss}’s character.` : ''}`;
    }
    const def = fallback(slot);
    if (!def) return `${row.name} has no main, so nothing is brought until you pick. This pick is for this timing only.`;
    const back = def.role === 'main' ? ` ${wide ? 'Choosing ' : ''}${def.ign} goes back to the main.` : '';
    return `Saves at once. Runs made from this timing use it unless the run has its own pick.${back}`;
  }

  /** The highest entry level above the character's level (or that one boss's): tinted and named, still offered. */
  function under(c: CharacterChoice, key?: string): { label: string; entry: number } | null {
    if (c.level === null) return null;
    let worst: { label: string; entry: number } | null = null;
    for (const { key: k, label, entry } of entries)
      if ((key === undefined || k === key) && entry !== null && entry > c.level && (!worst || entry > worst.entry)) worst = { label, entry };
    return worst;
  }

  /** One boss line's verdict, for the character that boss brings. */
  const verdictOf = (row: Row, i: number) => {
    const key = row.slot?.bosses[i]?.key;
    return row.ready?.bosses.find((b) => b.boss === key) ?? row.ready?.bosses[i] ?? null;
  };

  /** Each distinct character the member brings, with its values line once (B_FixedBoss2). */
  function charLines(row: Row): { ign: string; values: string; source: string }[] {
    if (!row.slot || !row.ready) return [];
    const r = row.ready;
    const ids = [...new Set(row.slot.bosses.map((b) => b.brings).filter((id): id is string => id !== null))];
    return ids.flatMap((id) => {
      const character = row.slot!.candidates.find((c) => c.id === id);
      const values = r.character?.id === id ? r : r.characters.find((c) => c.character.id === id);
      if (!character || !values) return [];
      return [{ ign: character.ign, ...readinessLine(values, scouterAt(r) !== null) }];
    });
  }
</script>

{#snippet basisTags(row: Row)}
  <span class="fixedready__tags">
    {#if row.unsaved}<span class="fixedready__basis fixedready__basis--new">not saved yet</span>{/if}
    {#if perBossOpen(row) && row.slot}
      {#if row.slot.per_boss}
        <span class="fixedready__basis fixedready__basis--picked" data-fid="fixed-ready-basis"><Icon name="layers" />per boss</span>
        <span>{new Set(row.slot.bosses.map((b) => b.brings)).size} characters · {row.slot.bosses.length} bosses</span>
      {:else}
        <span>{row.count} characters</span>
      {/if}
    {:else if row.character && row.basis === 'timing_pick'}
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
  {@const expanded = pickFor === keyOf(row.id, 0) || (sheetOpen && sheetFor?.member === row.id && sheetFor.i === 0)}
  {#if row.character}
    <button
      type="button"
      class="slotchar__btn slotchar__chip"
      data-fid="fixed-ready-char"
      data-char-for={keyOf(row.id, 0)}
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
      data-char-for={keyOf(row.id, 0)}
      aria-haspopup="dialog"
      aria-expanded={why ? undefined : expanded}
      aria-disabled={why || !row.slot ? 'true' : undefined}
      aria-label={why ? `${row.name} has no main. ${why}` : `Pick ${row.name}'s character for this timing: ${row.count} to choose from`}
      onclick={() => open(row)}>Pick character<span class="slotchar__count mono">{row.count}</span>{#if !why}<Icon name="chevron-down" />{/if}</button
    >
  {/if}
{/snippet}

{#snippet picker(row: Row, s: RunCharacterSlot, i: number)}
  {@const slot = s}
  {@const entry = i > 0 ? slot.bosses[i] : null}
  <CharacterPicker
    variant="popover"
    what="timing"
    fid="fixed-pick-menu"
    optionFid="fixed-pick-option"
    tagEveryOption
    {...{ slot: s }}
    name={row.name}
    {title}
    {busy}
    boss={entry ? { token: entry.boss, first: firstBoss, brings: entry.brings, picked: entry.picked } : null}
    meta={(c) => meta(slot, i, c)}
    under={(c) => under(c, entry?.key) !== null}
    onpick={(id) => choose(row, i, id)}
    onclose={(refocus) => void closePick(row.id, i, refocus)}
  >
    {#snippet note()}{@render pickNote(row, slot, i)}{/snippet}
  </CharacterPicker>
{/snippet}

{#snippet pickNote(row: Row, slot: RunCharacterSlot, i: number)}
  {#if wide}<span class="charpick__sep" aria-hidden="true"></span>{/if}
  <p class="charpick__note" data-fid="fixed-pick-note">{pickWords(row, slot, i)}</p>
{/snippet}

{#snippet toggle(row: Row)}
  {#if row.slot?.per_boss}
    <button
      type="button"
      class="btn bosspick__toggle"
      data-fid="fixed-boss-same"
      disabled={busy}
      aria-label="{row.name}: same character for every boss (clears the per-boss picks)"
      onclick={() => void same(row)}><Icon name="rotate-ccw" />Same for every boss</button
    >
  {:else}
    {@const isOpen = opened.includes(row.id)}
    <button
      type="button"
      class="btn bosspick__toggle"
      data-fid="fixed-boss-toggle"
      aria-expanded={isOpen}
      aria-label="Pick {row.name}'s character per boss"
      onclick={() => (opened = isOpen ? opened.filter((id) => id !== row.id) : [...opened, row.id])}><Icon name="layers" />Per boss<Icon name="chevron-down" /></button
    >
  {/if}
{/snippet}

<!-- B_FixedBoss2: one line per boss (boss · chip · verdict · basis), then each character's values once. -->
{#snippet bossList(row: Row, slot: RunCharacterSlot)}
  <ul class="bosspick" aria-label="{row.name}'s character per boss" data-fid="fixed-boss-picks">
    {#each slot.bosses as entry, i (entry.boss)}
      {@const character = slot.candidates.find((c) => c.id === entry.brings) ?? null}
      {@const inherited = i > 0 && entry.basis === 'same_as_first'}
      {@const verdict = verdictOf(row, i)}
      {@const key = keyOf(row.id, i)}
      <li class="bosspick__row" data-fid={i === 1 ? 'fixed-boss-row' : undefined}>
        <span class="bosspick__boss" title={entry.boss}>{entry.boss}</span>
        <span class="slotchar fixedready__control">
          <button
            type="button"
            class="slotchar__btn {character ? 'slotchar__chip' : 'slotchar__pick'}"
            class:slotchar__btn--same={inherited}
            data-char-for={key}
            aria-haspopup="dialog"
            aria-expanded={pickFor === key || (sheetOpen && sheetFor?.member === row.id && sheetFor.i === i)}
            aria-label={character
              ? `${row.name}'s character for ${entry.boss}: ${character.ign}${inherited ? `, same as ${firstBoss}` : ''}, change`
              : `Pick ${row.name}'s character for ${entry.boss}: ${slot.candidates.length} to choose from`}
            onclick={() => open(row, i)}
            >{#if character}<span class="slotchar__ign">{character.ign}</span>{#if wide && character.class}<span class="slotchar__meta">{character.class}</span>{/if}{:else}Pick character<span
                class="slotchar__count mono">{slot.candidates.length}</span
              >{/if}<Icon name="chevron-down" /></button
          >
          {#if wide && pickFor === key}{@render picker(row, slot, i)}{/if}
        </span>
        <span class="bosspick__chips">
          {#if verdict && character}{@const v = levelChip(verdict)}<StatusChip tone={v.tone}>{v.text}</StatusChip>{/if}
          {#if i === 0}
            {#if entry.basis === 'timing_pick'}<span class="fixedready__basis fixedready__basis--picked"><Icon name="pin" />picked for this timing</span>
            {:else if entry.basis === 'main'}<span class="fixedready__basis fixedready__basis--main"><Icon name="crown" />main</span>
            {:else if entry.basis === 'only'}<span class="fixedready__basis fixedready__basis--only">only character</span>{/if}
          {:else if inherited}
            <span class="fixedready__basis fixedready__basis--none">same as {firstBoss}</span>
          {:else if entry.basis === 'timing_pick'}
            <span class="fixedready__basis fixedready__basis--picked"><Icon name="pin" />picked for {entry.boss}</span>
          {/if}
        </span>
      </li>
    {/each}
  </ul>
  {@const lines = charLines(row)}
  {#if lines.length && !error}
    <div class="bosspick__chars" role="group" aria-label="Readiness of {lines.map((l) => l.ign).join(' and ')}" data-fid="fixed-ready-values">
      {#each lines as line (line.ign)}
        <span class="ready__line"
          ><span class="ready__values"><b>{line.ign}</b>{line.values}</span><span class="ready__source">{line.source}</span></span
        >
      {/each}
    </div>
  {/if}
{/snippet}

<div class="fixedready" class:fixedready--wide={wide} bind:this={panel} data-fid="fixed-ready">
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
        {@const many = perBossReady(row) && (row.count ?? 0) > 1}
        {@const listed = perBossOpen(row)}
        <li class="fixedready__row" class:fixedready__row--new={row.unsaved} data-fid="fixed-ready-slot">
          <div class="fixedready__head">
            <span class="fixedready__who"><b class="fixedready__name">{row.name}</b>{@render basisTags(row)}</span>
            <span class="slotchar fixedready__control">
              {#if listed && many}
                {@render toggle(row)}
              {:else if !listed}
                {@render control(row)}
                {#if wide && pickFor === keyOf(row.id, 0) && row.slot}{@render picker(row, row.slot, 0)}{/if}
              {/if}
            </span>
          </div>
          {#if listed && row.slot}
            {@render bossList(row, row.slot)}
          {:else if row.character && row.ready?.character?.id === row.character.id && !error}
            <!-- Values only for the character the row shows: right after a pick they wait for the next read. -->
            <ReadinessValues
              slot={row.ready}
              character={row.ready.character}
              {timeZone}
              {bossLabel}
              fid="fixed-ready-values"
              freshDays={ready.data?.fresh_days ?? null}
              bossesFid="fixed-ready-bosses"
              row={wide}
            />
          {:else if row.character && ready.loading && !error}
            <span class="fixedready__skel fixedready__skel--long"></span><span class="fixedready__skel fixedready__skel--short"></span>
          {/if}
          {#if !listed && many}{@render toggle(row)}{/if}
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
    {#if wide}
      Each member brings their main, else their only character. A pick here overrides that for this timing only and saves at once. <b>Per boss:</b> a boss with no pick
      of its own takes the first boss’s{firstBoss ? ` (${firstBoss}’s)` : ''} character. Runs made from this timing use these unless the run has its own pick.
    {:else}
      Each member brings their main, else their only character. A pick here saves at once; runs from this timing use it. Per boss: a boss with no pick of its own takes
      {firstBoss ? `${firstBoss}’s` : 'the first boss’s'} character.
    {/if}
  </p>
  <p class="fixedready__hint" data-fid="fixed-ready-rules">
    {#snippet levels()}{#each entries as e, i (e.label)}{i ? ', ' : ''}{e.label}
        {#if e.entry !== null}<span class="mono">Lv. {e.entry}</span>{:else}no entry level{/if}{/each}{/snippet}
    {#if wide}
      Per boss of the timing: level against its entry level{#if entries.length}&nbsp;({@render levels()}){/if}, for the character that boss brings. CP, or the estimate
      where a lookup is kept (it replaces CP), is shown, never judged. Fresh NEXON data first; else the roster level, marked manual.
    {:else}
      Level against each boss’s entry level{#if entries.length}: {@render levels()}{/if}. CP and estimates are shown, never judged.
    {/if}
  </p>
  <!-- Said once, and kept out of the visible notice so its Undo stays a plain button. -->
  <span class="vh" role="status" aria-live="polite" aria-atomic="true">{announcement}</span>
</div>

{#if !wide}
  <!-- The phone's pick sheet is a modal of its own beside the editor's sheet (B_PhoneFixedBossPick),
       not nested in it: the host moves to the body once, before any sheet opens in it. -->
  <div class="fixedready__sheethost" {@attach (host) => void document.body.append(host)}>
    {#if sheetRow?.slot && sheetFor}
      {@const row = sheetRow}
      {@const slot = sheetRow.slot}
      {@const i = sheetFor.i}
      {@const entry = i > 0 ? slot.bosses[i] : null}
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
        boss={entry ? { token: entry.boss, first: firstBoss, brings: entry.brings, picked: entry.picked } : null}
        meta={(c) => meta(slot, i, c)}
        under={(c) => under(c, entry?.key) !== null}
        onpick={(id) => choose(row, i, id)}
        returnFocus={() => trigger(row.id, i)}
      >
        {#snippet note()}{@render pickNote(row, slot, i)}{/snippet}
      </CharacterPicker>
    {/if}
  </div>
{/if}
