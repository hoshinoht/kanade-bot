<!--
  Config › CP floors (module `character_data`; boards B_CfgCpFloors,
  B_PhoneCfgCpFloors): the guild's combat power floor per boss and
  difficulty, typed in millions with one decimal and stored as whole CP.
  The run sheet's readiness judges CP against them, per boss of the run, and
  never blocks; a pair with no floor is shown but not judged. Entry levels
  come from boss knowledge and are read-only. Saved whole in one `cp_floors`
  PATCH (the map is replaced); refusals stay inline.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import type { Boss, BossRow, CpFloors, Difficulty } from '@kanade/api-types';
  import { DIFFICULTY_WORDS, Icon, Portrait, Select } from '@kanade/ui';
  import '@kanade/ui/styles/core/select.scss';
  import '@kanade/ui/styles/screens/character-data.scss';
  import { combatPower, floorText, parseFloor } from '../chardata/format';
  import { Resource } from '../resource.svelte';
  import { changes } from './dirty';
  import { followSaved } from './follow.svelte';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

  let { cpFloors, save }: { cpFloors: CpFloors; save: Save } = $props();
  const uid = $props.id();

  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  $effect(() => void bosses.load());

  interface Row {
    boss: string;
    difficulty: Difficulty;
    /** As typed, in millions. */
    text: string;
  }

  const rowsOf = (floors: CpFloors['floors']): Row[] =>
    Object.entries(floors).flatMap(([boss, byDifficulty]) =>
      Object.entries(byDifficulty).map(([difficulty, cp]) => ({ boss, difficulty: difficulty as Difficulty, text: floorText(cp as number) })),
    );

  // svelte-ignore state_referenced_locally
  let draft = $state(rowsOf(cpFloors.floors));
  followSaved(
    () => rowsOf(cpFloors.floors),
    () => draft,
    (saved) => (draft = saved),
  );
  let error = $state('');
  let bad = $state<number | null>(null);
  let saving = $state(false);
  let adding = $state('');
  let list: HTMLTableSectionElement | undefined = $state();
  let addPick: HTMLDivElement | undefined = $state();

  const rowOf = (key: string) => bosses.data?.find((b) => b.key === key);
  const nameOf = (key: string, d: Difficulty) => `${rowOf(key)?.name ?? key} ${DIFFICULTY_WORDS[d]}`;
  // Catalog order (the boss list's), so the table reads as the Bosses page does.
  const order = (key: string) => bosses.data?.findIndex((b) => b.key === key) ?? 0;
  const shown = $derived(draft.map((row, index) => ({ row, index })).sort((a, b) => order(a.row.boss) - order(b.row.boss) || a.row.difficulty.localeCompare(b.row.difficulty)));

  // The portrait wants a run's boss; the catalog row has what it draws.
  function portraitOf(key: string, difficulty: Difficulty): Boss | null {
    const row = rowOf(key);
    if (!row) return null;
    return { token: key, key, name: row.name, difficulty, level: row.level, portrait: row.portrait, portrait_sm: row.portrait, art: null, animated: null, hue: row.hue };
  }

  const savedText = (boss: string, d: Difficulty) => {
    const cp = cpFloors.floors[boss]?.[d];
    return cp === undefined ? '' : `${combatPower(cp)}`;
  };
  const pending = $derived.by(() => {
    const all = [...rowsOf(cpFloors.floors), ...draft].map((r) => `${r.boss}\u001f${r.difficulty}`);
    return changes(
      all.filter((key, i) => all.indexOf(key) === i).map((key) => {
        const [boss, d] = key.split('\u001f') as [string, Difficulty];
        const row = draft.find((r) => r.boss === boss && r.difficulty === d);
        const cp = row ? parseFloor(row.text) : null;
        return { label: nameOf(boss, d), from: savedText(boss, d), to: row ? (cp === null ? row.text : combatPower(cp)) : '' };
      }),
    );
  });

  // Every catalog pair a floor may name that has none yet, for Add a floor.
  const free = $derived(
    Object.entries(cpFloors.entry_levels).flatMap(([boss, byDifficulty]) =>
      Object.entries(byDifficulty)
        .filter(([d]) => !draft.some((r) => r.boss === boss && r.difficulty === d))
        .map(([d, entry]) => ({ value: `${boss}\u001f${d}`, label: `${nameOf(boss, d as Difficulty)}${entry ? ` · Lv. ${entry}` : ''}` })),
    ),
  );

  function discard() {
    draft = rowsOf(cpFloors.floors);
    error = '';
    bad = null;
  }

  async function add(value: string) {
    adding = '';
    const [boss, difficulty] = value.split('\u001f') as [string, Difficulty];
    if (!boss) return;
    draft.push({ boss, difficulty, text: '' });
    await tick();
    list?.querySelector<HTMLInputElement>(`[data-pair="${boss}-${difficulty}"] input`)?.focus();
  }

  async function remove(index: number) {
    draft.splice(index, 1);
    await tick();
    addPick?.querySelector<HTMLElement>('button')?.focus();
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const floors: CpFloors['floors'] = {};
    for (const [index, row] of draft.entries()) {
      const cp = parseFloor(row.text);
      if (cp === null) {
        bad = index;
        error = `${nameOf(row.boss, row.difficulty)}: a floor is millions with at most one decimal, from 0.1 to 10,000.`;
        list?.querySelector<HTMLInputElement>(`[data-pair="${row.boss}-${row.difficulty}"] input`)?.focus();
        return;
      }
      (floors[row.boss] ??= {})[row.difficulty] = cp;
    }
    bad = null;
    saving = true;
    error = await save({ cp_floors: { floors } }, 'CP floors saved; the run sheet judges by them at once.');
    saving = false;
    if (!error) draft = rowsOf(cpFloors.floors);
  }
</script>

<SettingsPanel title="CP floors">
  {#snippet lead()}Readiness on the run sheet compares a character’s combat power with these floors. It never blocks a pick or a save.{/snippet}
  <form id="{uid}-form" onsubmit={submit} aria-label="CP floors" novalidate>
    <fieldset class="settings__card cpfloors" data-fid="cpfloor-card">
      <legend class="settings__cardtitle">Floors</legend>
      {#if draft.length}
        <table class="settings__table cpfloors__table" data-fid="cpfloor-table">
          <caption class="vh">CP floors</caption>
          <thead>
            <tr><th scope="col">Boss</th><th scope="col">Difficulty</th><th scope="col">Entry level</th><th scope="col">CP floor</th><th scope="col"><span class="vh">Remove</span></th></tr>
          </thead>
          <tbody bind:this={list}>
            {#each shown as { row, index } (`${row.boss}-${row.difficulty}`)}
              {@const portrait = portraitOf(row.boss, row.difficulty)}
              {@const entry = cpFloors.entry_levels[row.boss]?.[row.difficulty] ?? null}
              {@const saved = cpFloors.floors[row.boss]?.[row.difficulty]}
              {@const changed = saved === undefined || parseFloor(row.text) !== saved}
              <tr data-fid="cpfloor-row" data-pair="{row.boss}-{row.difficulty}" class:cpfloors__row--bad={bad === index}>
                <th scope="row"><span class="cpfloors__boss">{#if portrait}<Portrait boss={portrait} />{/if}<b>{rowOf(row.boss)?.name ?? row.boss}</b></span></th>
                <td><span class="pill pill--{row.difficulty}">{DIFFICULTY_WORDS[row.difficulty].toUpperCase()}</span></td>
                <td class="mono cpfloors__entry">{entry === null ? '—' : `Lv. ${entry}`}</td>
                <td>
                  <span class="cpfloors__cp">
                    <input
                      class="mono cpfloors__input"
                      class:cpfloors__input--changed={changed}
                      inputmode="decimal"
                      autocomplete="off"
                      bind:value={row.text}
                      aria-label="CP floor for {nameOf(row.boss, row.difficulty)}, in millions"
                      aria-invalid={bad === index ? 'true' : undefined}
                      aria-describedby={bad === index ? `${uid}-error` : undefined}
                    />
                    <span>million</span>
                    {#if changed && saved !== undefined}<span class="savebar__dot" aria-hidden="true"></span><span class="chardata__sub">was <span class="mono">{combatPower(saved)}</span></span>{/if}
                  </span>
                </td>
                <td class="cpfloors__act">
                  <button type="button" class="btn btn--ghost" aria-label="Remove the {nameOf(row.boss, row.difficulty)} floor" onclick={() => remove(index)}>Remove</button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <p class="note">No floors yet: every boss’s CP shows on the run sheet but isn’t judged.</p>
      {/if}
      <div class="cpfloors__add" bind:this={addPick}>
        <span class="cpfloors__pick" data-fid="cpfloor-add">
          <Select
            label="Add a floor"
            bind:value={adding}
            options={free}
            placeholder="Add a floor"
            noun="bosses"
            disabled={!bosses.data || !free.length}
            onchange={(value) => void add(value)}
          />
        </span>
        <span class="chardata__sub">Any boss and difficulty not listed has <b>no floor</b>: its CP shows on the run sheet but isn’t judged.</span>
      </div>
      {#if error}<p class="field__error" role="alert" id="{uid}-error"><Icon name="alert-circle" />{error}</p>{/if}
    </fieldset>
    <p class="settings__box" data-fid="cpfloor-note">
      <Icon name="info" /><span>Combat power is NEXON’s number for the character, as of the date on each line. Entry level comes from boss knowledge; it can’t be edited here.</span>
    </p>
  </form>
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save CP floors" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>
