<!--
  Run lengths (workplan step planner-time-drops): how long one boss takes by
  default, and per boss + difficulty overrides. A run's length is the sum of
  its bosses'; the planner places dropped runs by it and steps keyboard moves
  by the default. Saved whole in one `run_lengths` PATCH; refusals stay
  inline and the success joins the toast like every other section.
-->
<script lang="ts">
  import type { BossRow, ConfigView, Difficulty } from '@kanade/api-types';
  import { tick } from 'svelte';
  import { Icon, Select } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { changes } from './dirty';
  import { Resource } from '../resource.svelte';
  import MinutesField from './MinutesField.svelte';
  import { check, DEFAULT_RANGE, draftOf, OVERRIDE_RANGE } from './runLengths';
  import { followSaved } from './follow.svelte';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

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
  followSaved(
    () => draftOf(runLengths),
    () => draft,
    (saved) => (draft = saved),
  );
  let error = $state('');
  let bad = $state<'default' | number | null>(null);
  let saving = $state(false);
  let list: HTMLUListElement | undefined = $state();
  let addButton: HTMLButtonElement | undefined = $state();

  const pending = $derived(
    changes([
      { label: 'Each boss', from: runLengths.default_minutes, to: draft.default_minutes, show: (v) => `${v ?? '—'} min` },
      {
        label: 'Overrides',
        from: draftOf(runLengths).overrides,
        to: draft.overrides,
        show: (v) => `${(v as unknown[]).length}`,
      },
    ]),
  );

  function discard() {
    draft = draftOf(runLengths);
    error = '';
    bad = null;
  }

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
    list?.querySelector<HTMLElement>('li:last-child button.dd, li:last-child select')?.focus();
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

<SettingsPanel title="Run lengths">
  {#snippet lead()}How long one boss takes; a run lasts the sum of its bosses' lengths.{/snippet}
<p class="settings__box">
  Dropping a run on the planner starts it right after the run above, and the arrow keys move a picked-up run by the default.
</p>
<form class="runlen" id="{uid}-form" onsubmit={submit} aria-label="Run lengths" novalidate>
  <fieldset class="settings__card" data-fid="cfg-card">
    <legend class="settings__cardtitle">Default</legend>
    <MinutesField label="Each boss" bind:value={draft.default_minutes} min={DEFAULT_RANGE.min} max={DEFAULT_RANGE.max} invalid={bad === 'default'} />
  </fieldset>

  <fieldset class="settings__card" data-fid="cfg-card">
    <legend class="settings__cardtitle">Overrides</legend>
    <p class="note">A boss and difficulty that takes longer (or shorter) than the default.</p>
    {#if draft.overrides.length}
      <ul class="runlen__list" bind:this={list}>
        {#each draft.overrides as row, index (index)}
          {@const name = row.boss ? (bossOf(row.boss)?.name ?? row.boss) : `Override ${index + 1}`}
          <li class="runlen__row" class:runlen__row--bad={bad === index}>
            <div class="field"
              ><span>Boss</span>
              <Select
                label="Boss"
                value={row.boss}
                options={[
                  ...(bosses.data ?? []).map((boss) => ({ value: boss.key, label: boss.name })),
                  ...(row.boss && !bossOf(row.boss) ? [{ value: row.boss, label: row.boss }] : []),
                ]}
                placeholder="Pick a boss"
                noun="bosses"
                invalid={bad === index && !row.boss}
                describedby={bad === index && (!row.boss || !row.difficulty) ? `${uid}-why-${index}` : undefined}
                onchange={(key) => pickBoss(index, key)}
              />
            </div>
            <div class="field"
              ><span>Difficulty</span>
              <Select
                label="Difficulty"
                bind:value={() => row.difficulty, (v) => (row.difficulty = v as Difficulty)}
                options={[
                  ...difficultiesOf(row.boss).map((d) => ({ value: d.letter, label: d.name })),
                  ...(row.difficulty && !difficultiesOf(row.boss).some((d) => d.letter === row.difficulty) ? [{ value: row.difficulty, label: row.difficulty }] : []),
                ]}
                placeholder={row.boss ? '—' : '— pick a boss first'}
                disabled={!row.boss}
                invalid={bad === index && Boolean(row.boss) && !row.difficulty}
                describedby={bad === index && (!row.boss || !row.difficulty) ? `${uid}-why-${index}` : undefined}
              />
            </div>
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
            {#if bad === index && (!row.boss || !row.difficulty)}
              <p class="dd-error runlen__why" id="{uid}-why-{index}">
                <Icon name="alert-circle" /><span>{row.boss ? 'Pick a difficulty' : 'Pick a boss'} before saving this length.</span>
              </p>
            {/if}
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
</form>
{#if error}<p class="field__error" role="alert">{error}</p>{/if}
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save run lengths" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>

<style>
  .runlen {
    display: grid;
    gap: 0.875rem;
  }

  .runlen legend {
    float: left;
    width: 100%;
    padding: 0;
  }

  .runlen legend + * {
    clear: both;
  }

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

  /* The words under the row's fields, so the fields keep one baseline. */
  .runlen__why {
    flex: 1 0 100%;
  }
</style>
