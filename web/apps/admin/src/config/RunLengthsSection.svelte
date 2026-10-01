<!--
  Run lengths (workplan step planner-time-drops): how long one boss takes by
  default, and per boss + difficulty overrides. A run's length is the sum of
  its bosses'; the planner places dropped runs by it and steps keyboard moves
  by the default. Saved whole in one `run_lengths` PATCH; refusals stay
  inline and the success joins the toast like every other section.
-->
<script lang="ts">
  import type { BossRow, ConfigView, Difficulty } from '@kanade/api-types';
  import { PendingLabel } from '@kanade/ui';
  import { tick } from 'svelte';
  import { Resource } from '../resource.svelte';
  import MinutesField from './MinutesField.svelte';
  import { check, DEFAULT_RANGE, draftOf, OVERRIDE_RANGE } from './runLengths';
  import type { Save } from './save';

  let {
    runLengths,
    save,
    onsaved,
  }: { runLengths: ConfigView['run_lengths']; save: Save; /** The saved default (the planner's keyboard step). */ onsaved?: (defaultMinutes: number) => void } =
    $props();
  const uid = $props.id();

  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  $effect(() => void bosses.load());

  // svelte-ignore state_referenced_locally
  let draft = $state(draftOf(runLengths));
  let error = $state('');
  let bad = $state<'default' | number | null>(null);
  let saving = $state(false);
  let list: HTMLUListElement | undefined = $state();
  let addButton: HTMLButtonElement | undefined = $state();

  const bossOf = (key: string) => bosses.data?.find((b) => b.key === key);
  const difficultiesOf = (key: string) => bossOf(key)?.difficulties ?? [];

  function pickBoss(index: number, key: string) {
    const row = draft.overrides[index]!;
    row.boss = key;
    // Keep the difficulty if the new boss has it; otherwise its first one.
    const options = difficultiesOf(key);
    if (!options.some((d) => d.letter === row.difficulty)) row.difficulty = options[0]?.letter ?? '';
  }

  async function add() {
    draft.overrides.push({ boss: '', difficulty: '', minutes: draft.default_minutes ?? 30 });
    await tick();
    list?.querySelector<HTMLSelectElement>('li:last-child select')?.focus();
  }

  async function remove(index: number) {
    draft.overrides.splice(index, 1);
    await tick();
    addButton?.focus();
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const checked = check(draft, bosses.data);
    if (!('value' in checked)) {
      error = checked.error;
      bad = checked.field ?? null;
      return;
    }
    bad = null;
    saving = true;
    error = await save({ run_lengths: checked.value }, 'Run lengths saved; the week shows the new lengths at its next refresh.');
    saving = false;
    // The saved object is the truth: resync (a refusal keeps the edits to fix).
    if (!error) {
      draft = draftOf(runLengths);
      onsaved?.(runLengths.default_minutes);
    }
  }
</script>

<h3 class="settings__title" id="{uid}-h">Run lengths</h3>
<p class="note">
  How long one boss takes. A run lasts the sum of its bosses' lengths: dropping a run on the planner starts it right after the run above, and the
  arrow keys move a picked-up run by the default.
</p>
<form onsubmit={submit} aria-labelledby="{uid}-h" novalidate>
  <fieldset class="settings__role">
    <legend>Default</legend>
    <MinutesField label="Each boss" bind:value={draft.default_minutes} min={DEFAULT_RANGE.min} max={DEFAULT_RANGE.max} invalid={bad === 'default'} />
  </fieldset>

  <fieldset class="settings__role">
    <legend>Overrides</legend>
    <p class="note">A boss and difficulty that takes longer (or shorter) than the default.</p>
    {#if draft.overrides.length}
      <ul class="runlen__list" bind:this={list}>
        {#each draft.overrides as row, index (index)}
          {@const name = row.boss ? (bossOf(row.boss)?.name ?? row.boss) : `Override ${index + 1}`}
          <li class="runlen__row" class:runlen__row--bad={bad === index}>
            <label class="field"
              ><span>Boss</span>
              <select value={row.boss} onchange={(event) => pickBoss(index, event.currentTarget.value)} aria-invalid={bad === index && !row.boss}>
                {#if !row.boss}<option value="">Pick a boss</option>{/if}
                {#each bosses.data ?? [] as boss (boss.key)}<option value={boss.key}>{boss.name}</option>{/each}
                {#if row.boss && !bossOf(row.boss)}<option value={row.boss}>{row.boss}</option>{/if}
              </select>
            </label>
            <label class="field"
              ><span>Difficulty</span>
              <select bind:value={row.difficulty} disabled={!row.boss} aria-invalid={bad === index && !row.difficulty}>
                {#if !row.difficulty}<option value="">—</option>{/if}
                {#each difficultiesOf(row.boss) as d (d.letter)}<option value={d.letter as Difficulty}>{d.name}</option>{/each}
                {#if row.difficulty && !difficultiesOf(row.boss).some((d) => d.letter === row.difficulty)}<option value={row.difficulty}
                    >{row.difficulty}</option
                  >{/if}
              </select>
            </label>
            <label class="field"
              ><span>Minutes</span>
              <input
                class="mono runlen__minutes"
                type="number"
                min={OVERRIDE_RANGE.min}
                max={OVERRIDE_RANGE.max}
                step="1"
                inputmode="numeric"
                bind:value={row.minutes}
                aria-invalid={bad === index}
              />
            </label>
            <button type="button" class="btn btn--ghost runlen__remove" aria-label="Remove the {name} override" onclick={() => remove(index)}>Remove</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="note">No overrides: every boss takes the default.</p>
    {/if}
    {#if bosses.error}<p class="field__error">Couldn't load the boss list: {bosses.error}</p>{/if}
    <div class="settings__actions">
      <button type="button" class="btn" bind:this={addButton} onclick={add} disabled={!bosses.data}>Add an override</button>
    </div>
  </fieldset>

  <div class="settings__actions">
    <button class="btn btn--primary" type="submit"><PendingLabel pending={saving} label="Saving…">Save run lengths</PendingLabel></button>
  </div>
</form>
<p class="field__error" role="alert">{error}</p>

<style>
  .runlen__list {
    display: grid;
    gap: 0.5rem;
    margin: 0.5rem 0 0;
    padding: 0;
    list-style: none;
  }

  .runlen__row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.4rem 0.7rem;
    padding: 0.5rem 0.7rem 0.6rem;
    border: 2px solid var(--line-soft);
    border-radius: var(--r-sm);
  }

  .runlen__row--bad {
    border-color: var(--risk);
  }

  .runlen__row .field + .field {
    margin-top: 0;
  }

  .runlen__minutes {
    width: 5.5rem;
    text-align: right;
  }

  .runlen__remove {
    margin-left: auto;
  }
</style>
