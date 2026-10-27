<!--
  The full run sheet's Party tab with characters (module `roster`; boards
  B_RunBossPicks*, B_RunBossPick, B_PhoneRunBossPicks, B_BossPickStates in
  docs/research/2026-10-16-per-boss-picks, the Party total row from
  2026-10-16-party-polish): a member × boss grid. The bosses are drawn once as
  columns; the first row is the Party total per boss (estimates on); then one
  row per member: answer mark, name, state and ×, and per boss the character
  chip (its own picker), that boss's level verdict and estimate %. A later
  boss with no pick of its own is faded and dashed, "same as <first boss>",
  so real overrides stand out; CP and its source show once per character,
  only when known; a clash warns in its cell, never blocking. "Same for every
  boss" ends a row whose run picks make the bosses differ. Four bosses or more
  scroll sideways with the member column frozen. The phone draws the Party
  total first, then a card per member with one line per boss. Picks save
  live (never staged), with Undo; a 409 `stale` re-reads and says so.
-->
<script lang="ts">
  import { tick, type Snippet } from 'svelte';
  import type { Boss, BossCharacterSlot, BossReadiness, CharacterChoice, CharacterClash, Participant, ReadinessSlot, Run, RunCharacterSlot, RunCharacters, RunReadiness, Week } from '@kanade/api-types';
  import { ANSWER_MARKS, asOfTime, assumedCrit, combatPower, DIFFICULTY_WORDS, hasTotal, Icon, olderThan, PartyTotal, PartyTotalFigure, PartyTotalNotes, Portrait, runTitle, StatusChip, totalNotes, type TotalReading } from '@kanade/ui';
  import CharacterPicker from './CharacterPicker.svelte';
  import { clashRun, runBasisTag, shownCharacter, type CharacterSlots } from './characters.svelte';
  import { levelChip } from '../chardata/format';
  import '@kanade/ui/styles/screens/share-chart.scss';

  type Notice = { ok: boolean; message: string; undo?: () => void };

  let {
    run,
    week,
    chars,
    ready,
    wide,
    busy,
    staging,
    who,
    picking = $bindable(false),
    onremove,
    onnotice,
    onleave,
    before,
    extra,
    tail,
    eyebrow,
  }: {
    run: Run;
    week: Week;
    chars: CharacterSlots<RunCharacters>;
    /** Module `character_data`: verdicts, estimates and CP; `null` without it. */
    ready: RunReadiness | null;
    wide: boolean;
    /** A party change is on its way: × waits. */
    busy: boolean;
    /** Run-sheet staging is on: picks still save live, and say so. */
    staging: boolean;
    who: (person: Participant) => string;
    /** A picker is open (presence shows the sheet as being edited). */
    picking?: boolean;
    onremove: (person: Participant) => void;
    onnotice: (notice: Notice) => void;
    /** "add on Members" leaves the sheet. */
    onleave: (event: MouseEvent) => void;
    /** The phone picker's line over its title: "HCarling + HMaleficStar · Tue 29 22:00". */
    eyebrow: string;
    /** The phone list's first items (the readiness note). */
    before?: Snippet;
    /** Staged additions and "Add someone…", in the grid's or the list's own markup. */
    extra?: Snippet;
    /** The phone list's last item (the Estimate panel). */
    tail?: Snippet;
  } = $props();

  const title = $derived(runTitle(run));
  const first = $derived(run.bosses[0]?.token ?? '');
  const total = $derived(chars.data?.party_total ?? null);
  const bossTitle = (boss: Boss) => `${boss.name} ${DIFFICULTY_WORDS[boss.difficulty]}`;
  const totalBosses = $derived(run.bosses.map((boss) => ({ token: boss.token, title: bossTitle(boss) })));
  const multi = $derived(run.bosses.length > 1);
  // An ended (frozen) run refuses party changes; character picks are not frozen.
  const endedWhy = $derived(run.ended ? 'This run has ended.' : undefined);

  type Cell = {
    i: number;
    boss: Boss;
    /** The slot's entry for this boss (absent on a run with no catalog bosses). */
    entry: BossCharacterSlot | null;
    character: CharacterChoice | null;
    /** A later boss with no pick of its own: the first boss's character. */
    inherited: boolean;
    verdict: BossReadiness | null;
    /** The character's first cell in the row: its CP line goes here. */
    lead: boolean;
    clashes: CharacterClash[];
  };

  const readyOf = (member: string): ReadinessSlot | null => ready?.slots.find((s) => s.member_id === member) ?? null;

  function cells(slot: RunCharacterSlot): Cell[] {
    const r = readyOf(slot.member_id);
    const seen: string[] = [];
    const out = run.bosses.map((boss, i): Cell => {
      const entry = slot.bosses[i] ?? null;
      const id = entry ? entry.brings : slot.brings;
      const character = slot.candidates.find((c) => c.id === id) ?? null;
      const lead = character !== null && !seen.includes(character.id);
      if (character) seen.push(character.id);
      return {
        i,
        boss,
        entry,
        character,
        inherited: i > 0 && entry?.basis === 'same_as_first',
        verdict: r?.bosses.find((b) => b.boss === boss.key && b.difficulty === boss.difficulty) ?? null,
        lead,
        clashes: [],
      };
    });
    // Each clash warns in the cell of the boss it shares, with that boss's character; else the first cell.
    for (const clash of slot.warnings) {
      const cell = out.find((c) => c.character?.id === clash.character_id && clash.bosses.some((token) => token.slice(1) === c.boss.key)) ?? out[0];
      cell?.clashes.push(clash);
    }
    return out;
  }

  /** "MikaBish · CP 48.2M · NEXON · 29 Sep": only while the CP is known. */
  function cpLine(member: string, character: CharacterChoice): string | null {
    const r = readyOf(member);
    const power = r?.character?.id === character.id ? r.power : (r?.characters.find((c) => c.character.id === character.id)?.power ?? null);
    if (!power) return null;
    const [, m, d] = (power.as_of ?? '').split('-').map(Number);
    const month = m ? new Intl.DateTimeFormat('en-US', { month: 'short', timeZone: 'UTC' }).format(new Date(Date.UTC(2000, m - 1, 1))) : '';
    return `${character.ign} · CP ${combatPower(power.value)} · NEXON${d && month ? ` · ${d} ${month}` : ''}`;
  }

  /**
   * Where the row's characters come from when the run did not settle it alone: "picked on this
   * run", "from timing", "HCarling picked on this run · HMaleficStar from timing". A boss that
   * follows the first boss's character follows its basis too.
   */
  function basisLine(slot: RunCharacterSlot, row: Cell[]): string {
    const words = (basis: string | null | undefined) => (basis === 'run_pick' ? 'picked on this run' : basis === 'timing_pick' ? 'from timing' : null);
    const own = row.filter((cell) => !cell.inherited);
    const parts = own.map((cell) => ({ token: cell.boss.token, word: words(cell.i === 0 ? slot.basis : cell.entry?.basis) })).filter((p) => p.word);
    if (!parts.length) return '';
    if (parts.length === own.length && parts.every((p) => p.word === parts[0]!.word)) return parts[0]!.word!;
    return parts.map((p) => `${p.token} ${p.word}`).join(' · ');
  }

  // "Same for every boss" clears the run's own later-boss picks; a difference that comes from the timing is the timing's to undo.
  const ownLater = (slot: RunCharacterSlot) => slot.bosses.slice(1).some((b) => b.basis === 'run_pick');

  const readings = $derived(
    run.participants.flatMap((person): TotalReading[] => {
      const slot = chars.slot(person.id);
      return slot
        ? cells(slot).map((cell) => ({ name: who(person), token: cell.boss.token, ign: cell.character?.ign ?? null, reading: cell.verdict?.scouter ?? null }))
        : [];
    }),
  );
  const notes = $derived(total ? totalNotes(total, readings) : { assumed: [], none: [] });

  // The wide sheet's popover (one cell's), the narrow sheet's bottom sheet.
  let popFor = $state<string | null>(null);
  let sheetFor = $state<{ member: string; i: number } | null>(null);
  let sheetOpen = $state(false);
  let saving = $state(false);
  let root = $state<HTMLElement>();
  $effect(() => {
    picking = popFor !== null || sheetOpen;
  });
  const cellKey = (member: string, i: number) => `${member}:${i}`;
  const trigger = (member: string, i: number) => root?.querySelector<HTMLElement>(`[data-char-for="${CSS.escape(cellKey(member, i))}"]`) ?? null;

  function open(member: string, i: number) {
    if (wide) popFor = popFor === cellKey(member, i) ? null : cellKey(member, i);
    else {
      sheetFor = { member, i };
      sheetOpen = true;
    }
  }

  async function closePop(member: string, i: number, refocus: boolean) {
    if (popFor === cellKey(member, i)) popFor = null;
    if (!refocus) return;
    await tick();
    trigger(member, i)?.focus({ preventScroll: true });
  }

  const tagOf = (slot: RunCharacterSlot, i: number) => {
    if (i === 0) return runBasisTag(slot.basis);
    const basis = slot.bosses[i]?.basis;
    return basis === 'same_as_first' ? `same as ${first}` : basis === 'timing_pick' ? 'from timing' : null;
  };

  /** Saves one cell's pick (`null` clears it); the first boss is the member's own pick. */
  async function choose(person: Participant, i: number, characterId: string | null, undoable = true) {
    const slot = chars.slot(person.id);
    if (!slot || saving) return;
    const entry = i > 0 ? slot.bosses[i] : null;
    const before = entry ? entry.picked : slot.picked;
    const current = entry ? entry.brings : slot.brings;
    if (characterId !== null && characterId === current && characterId === before) {
      popFor = null;
      sheetOpen = false;
      return;
    }
    const label = who(person);
    const token = run.bosses[i]?.token ?? title;
    saving = true;
    const result = await (entry ? chars.pickBoss(person.id, entry.key, characterId) : chars.pick(person.id, characterId)).finally(() => (saving = false));
    popFor = null;
    sheetOpen = false;
    if (result.ok) {
      const now = result.value;
      const shown = i > 0 ? (now.candidates.find((c) => c.id === now.bosses[i]?.brings) ?? null) : shownCharacter(now);
      const tag = tagOf(now, i);
      const where = i > 0 ? token : title;
      const done = characterId
        ? `${label} brings ${shown?.ign ?? 'that character'} to ${where}.`
        : shown
          ? `Cleared ${label}'s ${i > 0 ? `${token} ` : ''}pick: back to ${shown.ign}${tag ? ` (${tag})` : ''}.`
          : `Cleared ${label}'s pick.`;
      onnotice({
        ok: true,
        message: staging ? `${done} Saved live, not in the draft.` : done,
        undo: undoable && before !== characterId ? () => void choose(person, i, before, false) : undefined,
      });
    } else if (result.code === 'stale') {
      onnotice({ ok: false, message: `${label}'s character ${i > 0 ? `for ${token}` : `on ${title}`} changed meanwhile; the ${i > 0 ? 'cell' : 'slot'} shows what is saved now.` });
    } else if (!result.unbuilt) {
      onnotice({ ok: false, message: `Couldn't set ${label}'s character: ${result.message}` });
    }
    await tick();
    trigger(person.id, i)?.focus({ preventScroll: true });
  }

  /** "Same for every boss": one write; Undo puts each later boss's own run pick back. */
  async function same(person: Participant) {
    const slot = chars.slot(person.id);
    if (!slot || saving) return;
    const kept = slot.bosses.slice(1).filter((b) => b.basis === 'run_pick' && b.picked);
    const label = who(person);
    saving = true;
    const result = await chars.same(person.id).finally(() => (saving = false));
    if (result.ok) {
      const shown = shownCharacter(result.value);
      const done = shown ? `${label} brings ${shown.ign} to every boss again.` : `${label}'s per-boss picks are cleared.`;
      onnotice({
        ok: true,
        message: staging ? `${done} Saved live, not in the draft.` : done,
        undo: kept.length ? () => void restore(person, kept) : undefined,
      });
    } else if (result.code === 'stale') {
      onnotice({ ok: false, message: `${label}'s per-boss characters changed meanwhile; the row shows what is saved now.` });
    } else if (!result.unbuilt) {
      onnotice({ ok: false, message: `Couldn't set ${label}'s characters: ${result.message}` });
    }
  }

  /**
   * Undo of "Same for every boss": each later boss's own run pick, back one by one. A failed one
   * stops there and keeps an Undo for it and the rest (a stale one has been re-read, so the retry
   * sends the version now saved); none is dropped silently.
   */
  async function restore(person: Participant, kept: BossCharacterSlot[]) {
    const label = who(person);
    saving = true;
    let done = 0;
    try {
      for (const b of kept) {
        const back = await chars.pickBoss(person.id, b.key, b.picked);
        if (!back.ok) {
          const left = kept.slice(done);
          onnotice({
            ok: false,
            message: `Put back ${done} of ${kept.length} of ${label}'s per-boss picks; ${b.boss} failed (${back.message}). Undo tries the ${left.length === 1 ? 'last one' : `other ${left.length}`} again.`,
            undo: () => void restore(person, left),
          });
          return;
        }
        done += 1;
      }
    } finally {
      saving = false;
    }
    onnotice({ ok: true, message: `${label}'s per-boss picks are back.` });
  }

  /** The picker's words after a choice's class: the first boss's character, and an entry level it is under. */
  function meta(i: number, slot: RunCharacterSlot, cell: Cell | undefined, c: CharacterChoice): string[] {
    const out: string[] = [];
    if (i > 0 && c.id === slot.brings) out.push(`same as ${first}`);
    const entry = cell?.verdict?.entry_level ?? null;
    if (i > 0 && entry !== null && c.level !== null && c.level < entry) out.push(`under ${cell!.boss.token}’s Lv. ${entry}`);
    return out;
  }
  const under = (cell: Cell | undefined, c: CharacterChoice) => {
    const entry = cell?.verdict?.entry_level ?? null;
    return entry !== null && c.level !== null && c.level < entry;
  };

  const sheetPerson = $derived(sheetFor ? (run.participants.find((p) => p.id === sheetFor!.member) ?? null) : null);
  const sheetSlot = $derived(sheetFor ? chars.slot(sheetFor.member) : null);
</script>

{#snippet chip(person: Participant, slot: RunCharacterSlot, cell: Cell)}
  {@const label = who(person)}
  {@const key = cellKey(person.id, cell.i)}
  {@const expanded = popFor === key || (sheetOpen && sheetFor?.member === person.id && sheetFor.i === cell.i)}
  {@const tag = tagOf(slot, cell.i)}
  <span class="slotchar">
    {#if cell.character && cell.i > 0 && slot.candidates.length === 1}
      <!-- One character: nothing to pick per boss. -->
      <span class="slotchar__btn slotchar__chip slotchar__static"><span class="slotchar__ign">{cell.character.ign}</span>{#if cell.character.level}<span class="slotchar__lv mono">{cell.character.level}</span>{/if}</span>
    {:else if cell.character}
      <button
        type="button"
        class="slotchar__btn slotchar__chip"
        data-char-for={key}
        aria-haspopup="dialog"
        aria-expanded={expanded}
        aria-label={cell.i === 0
          ? `${label}'s character on ${title}: ${cell.character.ign}${tag ? `, ${tag}` : ''}. Change`
          : `${label}'s character for ${cell.boss.token}: ${cell.character.ign}${tag ? `, ${tag}` : ''}. Change`}
        onclick={() => open(person.id, cell.i)}
        ><span class="slotchar__ign">{cell.character.ign}</span>{#if cell.character.level}<span class="slotchar__lv mono">{cell.character.level}</span>{/if}{#if wide}<Icon
            name="chevron-down"
          />{/if}</button
      >
    {:else}
      <button
        type="button"
        class="slotchar__btn slotchar__pick"
        data-char-for={key}
        aria-haspopup="dialog"
        aria-expanded={expanded}
        aria-label="Pick {label}'s character {cell.i === 0 ? `on ${title}` : `for ${cell.boss.token}`}: {slot.candidates.length} to choose from"
        onclick={() => open(person.id, cell.i)}
        >Pick<span class="slotchar__count mono">{slot.candidates.length}</span>{#if wide}<Icon name="chevron-down" />{/if}</button
      >
    {/if}
    {#if wide && popFor === key}
      <CharacterPicker
        variant="popover"
        {slot}
        name={label}
        {title}
        busy={saving}
        {staging}
        fid={cell.i > 0 ? 'party-pick-menu' : 'roster-picker'}
        boss={cell.i > 0 && cell.entry ? { token: cell.boss.token, first, brings: cell.entry.brings, picked: cell.entry.picked } : null}
        meta={(c) => meta(cell.i, slot, cell, c)}
        under={(c) => cell.i > 0 && under(cell, c)}
        onpick={(id) => void choose(person, cell.i, id)}
        onclose={(refocus) => void closePop(person.id, cell.i, refocus)}
      >
        {#snippet note()}{#if cell.i > 0}<p class="charpick__note">For this run only. A boss with no pick of its own brings the timing’s pick for it, else {first}’s character.</p>{/if}{/snippet}
      </CharacterPicker>
    {/if}
  </span>
{/snippet}

{#snippet facts(cell: Cell)}
  <span class="bossgrid__facts">
    {#if cell.verdict && cell.character}
      {@const v = levelChip(cell.verdict)}
      <StatusChip tone={v.tone}>{v.text}</StatusChip>
      {#if cell.verdict.scouter}
        {@const reading = cell.verdict.scouter}
        {@const assumed = assumedCrit(reading)}
        <span class="bossgrid__pct" title="Estimate: {cell.character.ign}’s % on {cell.boss.token} · {reading.rating}{assumed ? ` · ${assumed}` : ''}{reading.stale && ready?.fresh_days ? olderThan(ready.fresh_days) : ''}"
          >{Math.round(reading.percentage)}%{assumed ? '*' : ''}<span class="vh"> estimate{assumed ? `, ${assumed}` : ''}</span></span
        >
      {/if}
    {/if}
    {#if cell.inherited}<span class="bossgrid__same">same as {first}</span>{/if}
  </span>
{/snippet}

{#snippet warns(cell: Cell)}
  {#each cell.clashes as clash (clash.run_id)}
    <span class="bossgrid__warn" data-fid="party-warn" role="note"
      ><Icon name="alert-triangle" /><span>Also on {clashRun(week, clash)} this boss week{clash.basis === 'run_pick' ? '' : ` (${runBasisTag(clash.basis)} there)`}.</span></span
    >
  {/each}
{/snippet}

{#snippet none(person: Participant)}
  <span class="bossgrid__empty slotchar__none"
    >{wide ? 'No characters yet' : 'no characters'} · <a
      href="/members?open={encodeURIComponent(person.id)}"
      aria-label="{who(person)} has no characters yet: add them on Members"
      onclick={onleave}>{wide ? 'add on Members' : 'add'}</a
    ></span
  >
{/snippet}

{#snippet sameButton(person: Participant, cls: string)}
  <button
    type="button"
    class="btn {cls}"
    data-fid="party-boss-same"
    disabled={saving}
    aria-label="{who(person)}: same character for every boss on this run"
    onclick={() => void same(person)}><Icon name="rotate-ccw" />Same for every boss</button
  >
{/snippet}

{#snippet asOf()}
  {#if total && !hasTotal(total)}
    <span class="share__sub">Look up party fills it in.</span>
  {:else if total?.as_of}
    <span class="share__sub" data-fid="party-total-asof"
      >as of <span class="mono">{asOfTime(total.as_of, week.timezone)}</span>{#if total.stale}&nbsp;· <span class="party-total__old">{olderThan(total.fresh_days).slice(3)}</span>{/if}</span
    >
  {/if}
{/snippet}

{#if wide}
  <div class="bossgrid-wrap" data-fid="week-party" bind:this={root}>
    <div class="bossgrid__scroll" class:bossgrid__scroll--wide={run.bosses.length >= 4}>
      <table class="bossgrid" class:bossgrid--wide={run.bosses.length >= 4} aria-label="Party: each member’s character per boss" {@attach (node: HTMLElement) => node.style.setProperty('--bosses', String(run.bosses.length))}>
        <colgroup>
          <col class="bossgrid__c-who" />
          {#each run.bosses as boss (boss.token)}<col />{/each}
          {#if multi}<col class="bossgrid__c-end" />{/if}
        </colgroup>
        <thead>
          <tr>
            <td class="bossgrid__corner">{run.participants.length} in the party</td>
            {#each run.bosses as boss (boss.token)}
              <th scope="col" data-fid="party-grid-boss">
                <span class="bossgrid__head"
                  ><Portrait {boss} /><b title={bossTitle(boss)}>{boss.name}</b><span class="bossgrid__meta"
                    ><span class="pill pill--{boss.difficulty}">{DIFFICULTY_WORDS[boss.difficulty].toUpperCase()}</span>{#if boss.level}<span class="boss__lv">Lv. {boss.level}</span>{/if}</span
                  ></span
                >
              </th>
            {/each}
            {#if multi}<td></td>{/if}
          </tr>
        </thead>
        <tbody>
          {#if total}
            <tr class="bossgrid__total" data-fid="party-total">
              <th scope="row"><span class="share__title">Party total</span>{@render asOf()}</th>
              {#each run.bosses as boss, i (boss.token)}
                {@const figure = total.bosses.find((b) => b.boss === boss.token)}
                <td data-fid={i === 0 ? 'party-total-row' : undefined}>{#if figure}<PartyTotalFigure boss={figure} />{:else}<span class="party-total__none">no estimate yet</span>{/if}</td>
              {/each}
              {#if multi}<td></td>{/if}
            </tr>
          {/if}
          {#each run.participants as person, k (person.id)}
            {@const slot = chars.slot(person.id)}
            {@const row = slot ? cells(slot) : []}
            {@const basis = slot ? basisLine(slot, row) : ''}
            <tr class="bossgrid__row" data-fid={k === 0 ? 'party-slot' : undefined}>
              <th scope="row" class="bossgrid__who bossgrid__who--{person.answer}">
                <span class="bossgrid__id"
                  ><span class="slot__mark" aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span><b class="slot__name">{who(person)}</b><span class="slot__state"
                    >{ANSWER_MARKS[person.answer].word}</span
                  ><button type="button" class="chip__x slot__x" aria-label="Take {who(person)} off this run for this week only" title={endedWhy} disabled={busy || run.ended} onclick={() => onremove(person)}
                    >×</button
                  ></span
                >{#if basis}<span class="bossgrid__basis">{basis}</span>{/if}
              </th>
              {#if slot && slot.candidates.length === 0}
                <td colspan={run.bosses.length + (multi ? 1 : 0)}>{@render none(person)}</td>
              {:else if slot}
                {#each row as cell (cell.boss.token)}
                  <td class:bossgrid--inherited={cell.inherited} data-fid={k === 0 && cell.i === 0 ? 'party-grid-cell' : undefined}>
                    <span class="bossgrid__cell">
                      {@render chip(person, slot, cell)}
                      {@render facts(cell)}
                      {#if cell.lead && cell.character}
                        {@const cp = cpLine(person.id, cell.character)}
                        {#if cp}<span class="bossgrid__src">{cp}</span>{/if}
                      {/if}
                      {@render warns(cell)}
                    </span>
                  </td>
                {/each}
                {#if multi}<td class="bossgrid__end">{#if slot.per_boss && ownLater(slot)}{@render sameButton(person, '')}{/if}</td>{/if}
              {:else}
                <td colspan={run.bosses.length + (multi ? 1 : 0)}></td>
              {/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if total && hasTotal(total)}<PartyTotalNotes {notes} fid="party-total-notes" />{/if}
    {@render extra?.()}
  </div>
{:else}
  <ul class="runsheet__party runsheet__party--chars" aria-label="Party" data-fid="week-party" bind:this={root}>
    {@render before?.()}
    {#if total}
      <PartyTotal tag="li" {total} bosses={totalBosses} timeZone={week.timezone} {notes} audience="admin" fids={{ row: 'party-total-row', asOf: 'party-total-asof', notes: 'party-total-notes' }} data-fid="party-total" />
    {/if}
    {#each run.participants as person, k (person.id)}
      {@const slot = chars.slot(person.id)}
      {@const row = slot ? cells(slot) : []}
      {@const basis = slot ? basisLine(slot, row) : ''}
      <li class="slot slot--{person.answer} slot--chars" data-fid={k === 0 ? 'party-slot' : undefined}>
        <span class="slot__head">
          <span class="slot__mark" aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span>
          <span class="slot__text"><b class="slot__name">{who(person)}</b><span class="slot__state">{ANSWER_MARKS[person.answer].word}</span></span>
          <button type="button" class="chip__x slot__x" aria-label="Take {who(person)} off this run for this week only" title={endedWhy} disabled={busy || run.ended} onclick={() => onremove(person)}>×</button>
        </span>
        {#if basis}<span class="bossgrid__basis">{basis}</span>{/if}
        {#if slot && slot.candidates.length === 0}
          {@render none(person)}
        {:else if slot}
          <ul class="bossstack" aria-label="{who(person)}'s character per boss">
            {#each row as cell (cell.boss.token)}
              <li class="bossstack__row" class:bossgrid--inherited={cell.inherited} data-fid={k === 0 && cell.i === 0 ? 'party-grid-cell' : undefined}>
                <span class="bossstack__boss" title={bossTitle(cell.boss)}>{cell.boss.token}</span>
                <span class="bossgrid__cell">
                  {@render chip(person, slot, cell)}
                  {@render facts(cell)}
                  {@render warns(cell)}
                </span>
              </li>
            {/each}
          </ul>
          {#each row.filter((cell) => cell.lead && cell.character) as cell (cell.character!.id)}
            {@const cp = cpLine(person.id, cell.character!)}
            {#if cp}<span class="bossgrid__src">{cp}</span>{/if}
          {/each}
          {#if multi && slot.per_boss && ownLater(slot)}{@render sameButton(person, 'bossgrid__reset')}{/if}
        {/if}
      </li>
    {/each}
    {@render extra?.()}
    {@render tail?.()}
  </ul>
  {#if sheetFor && sheetSlot && sheetPerson}
    {@const i = sheetFor.i}
    {@const slot = sheetSlot}
    {@const person = sheetPerson}
    {@const cell = cells(slot)[i]}
    <CharacterPicker
      variant="sheet"
      bind:open={sheetOpen}
      {slot}
      name={who(person)}
      {title}
      {eyebrow}
      busy={saving}
      {staging}
      fid={i > 0 ? 'party-pick-menu' : 'roster-picker'}
      boss={i > 0 && cell?.entry ? { token: cell.boss.token, first, brings: cell.entry.brings, picked: cell.entry.picked } : null}
      meta={(c) => meta(i, slot, cell, c)}
      under={(c) => i > 0 && under(cell, c)}
      onpick={(id) => void choose(person, i, id)}
      returnFocus={() => trigger(person.id, i)}
    />
  {/if}
{/if}
