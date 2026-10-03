<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { LiveRegion } from '@kanade/ui';
  import { tick } from 'svelte';
  import { changes } from './dirty';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

  let { pings, save }: { pings: ConfigView['pings']; save: Save } = $props();
  const uid = $props.id();

  // Seeded from the saved values once; a refused save keeps what was typed.
  // svelte-ignore state_referenced_locally
  let time = $state(pings.day_of_ping_time);
  // svelte-ignore state_referenced_locally
  let minutes = $state<number[]>([...pings.countdown_minutes]);
  /** The add field; a typed value counts as drafted (B_CfgPings) even before Add. */
  let draft = $state('');
  let error = $state('');
  let saving = $state(false);
  let announce = $state('');
  // Only the field that failed validation carries aria-invalid.
  let badTime = $state(false);
  let badCountdowns = $state(false);
  let addField: HTMLInputElement | undefined = $state();
  let chipList: HTMLUListElement | undefined = $state();

  // The comma field's rule: whole minutes, split on commas or spaces.
  const parts = (value: string) => value.split(/[\s,]+/).filter(Boolean);
  const wholeMinutes = (value: string) => parts(value).every((n) => /^\d+$/.test(n));
  /** The list with the typed minutes appended, each value once, in the order given. */
  const merged = (list: number[], text: string) => [...new Set([...list, ...parts(text).map(Number)])];
  const drafted = $derived(wholeMinutes(draft) ? merged(minutes, draft) : minutes);

  const pending = $derived(
    changes([
      { label: 'Morning ping', from: pings.day_of_ping_time, to: time.trim() },
      { label: 'Countdowns', from: pings.countdown_minutes.join(', '), to: (draft.trim() && !wholeMinutes(draft) ? [...minutes, draft.trim()] : drafted).join(', ') },
    ]),
  );

  function discard() {
    time = pings.day_of_ping_time;
    minutes = [...pings.countdown_minutes];
    draft = '';
    error = '';
    badTime = badCountdowns = false;
  }

  const validTime = (value: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(value.trim());
  const COUNTDOWN_RULE = 'Countdowns are whole minutes, separated by commas.';

  /** Moves the add field's minutes into chips; false (and the rule shown) when they are not whole minutes. */
  function add(): boolean {
    if (!draft.trim()) return true;
    if (!wholeMinutes(draft)) {
      badCountdowns = true;
      error = COUNTDOWN_RULE;
      return false;
    }
    const next = merged(minutes, draft);
    const fresh = next.slice(minutes.length);
    const repeated = parts(draft).length - fresh.length;
    minutes = next;
    draft = '';
    badCountdowns = false;
    if (error === COUNTDOWN_RULE) error = '';
    announce = [
      fresh.length ? `Added ${fresh.join(', ')} min.` : '',
      repeated ? `${repeated === 1 ? 'That countdown is' : 'Those countdowns are'} already listed.` : '',
    ]
      .filter(Boolean)
      .join(' ');
    return true;
  }

  async function remove(index: number) {
    const [gone] = minutes.splice(index, 1);
    announce = `Removed ${gone} min. ${minutes.length ? `Countdowns ${minutes.join(', ')} min.` : 'No countdowns left.'}`;
    await tick();
    // Focus stays in the list: the next chip's ×, else the previous one, else the add field.
    const left = chipList?.querySelectorAll<HTMLButtonElement>('.pings__remove') ?? [];
    (left[Math.min(index, left.length - 1)] ?? addField)?.focus({ preventScroll: true });
  }

  function addKeydown(event: KeyboardEvent) {
    if (event.key !== 'Enter') return;
    // Enter adds what was typed; with nothing typed it submits the form as before.
    if (!draft.trim()) return;
    event.preventDefault();
    add();
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    badTime = !validTime(time);
    const added = add();
    badCountdowns = !added || minutes.length === 0;
    if (badTime || badCountdowns) {
      error = badTime ? 'The morning ping is HH:MM, for example 09:00.' : COUNTDOWN_RULE;
      return;
    }
    saving = true;
    error = await save({ pings: { day_of_ping_time: time.trim(), countdown_minutes: [...minutes] } }, 'Pings saved; they take effect when the bot restarts.');
    saving = false;
    badTime = error.includes('HH:MM');
    badCountdowns = !badTime && error.includes('Countdown');
  }
</script>

<SettingsPanel title="Pings">
  {#snippet lead()}Reminder timing for every run.{/snippet}
  <form class="settings__card" data-fid="cfg-card" id="{uid}-form" onsubmit={submit} aria-label="Pings" aria-describedby="{uid}-note">
    <div class="pings">
      <div class="pings__label"><b>Morning ping</b><span class="settings__cardnote">day-of card for every run</span></div>
      <label class="pings__field"
        ><span class="vh">Morning ping</span><input
          class="mono pings__time"
          class:settings__changed={time.trim() !== pings.day_of_ping_time}
          bind:value={time}
          size="6"
          inputmode="numeric"
          autocomplete="off"
          aria-invalid={badTime}
        /><span class="settings__cardnote">guild time · 24-hour</span></label
      >
      <div class="pings__label" id="{uid}-cd"><b>Countdowns</b><span class="settings__cardnote">minutes before start</span></div>
      <div class="pings__field" role="group" aria-labelledby="{uid}-cd">
        <ul class="pings__chips" aria-label="Countdowns" bind:this={chipList}>
          {#each minutes as minute, index (`${index}:${minute}`)}
            <li class="pings__chip mono">
              {minute} min<button
                class="pings__remove"
                type="button"
                aria-label="Remove the {minute} min countdown"
                onclick={() => void remove(index)}><span aria-hidden="true">×</span></button
              >
            </li>
          {/each}
        </ul>
        <label class="pings__add"
          ><span class="vh">Add a countdown (minutes)</span><input
            class="mono pings__addfield"
            class:settings__changed={draft.trim() !== ''}
            bind:this={addField}
            bind:value={draft}
            onkeydown={addKeydown}
            size="4"
            inputmode="numeric"
            autocomplete="off"
            aria-invalid={badCountdowns}
          /></label
        ><button class="btn pings__addbtn" type="button" onclick={() => add()}>Add</button>
      </div>
    </div>
  </form>
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <LiveRegion message={announce} />
  <p class="settings__box" id="{uid}-note">The server applies pings when the bot restarts; then every ping that has not fired yet is re-placed.</p>
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save pings" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>

<style>
  .pings {
    display: grid;
    grid-template-columns: 12.5rem minmax(0, 1fr);
    align-items: center;
    gap: 1rem 1.25rem;
  }

  .pings__label b {
    font-size: var(--fs-body-sm);
  }

  .pings__label {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pings__field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.625rem;
  }

  .pings__time {
    width: 6.25rem;
  }

  /* Countdown chips (the board's .alias): mono, tonal, a round × at the end. */
  .pings__chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .pings__field[role='group'] {
    gap: 6px;
  }

  .pings__chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 26px;
    margin: 0;
    padding: 0 2px 0 10px;
    border-radius: 13px;
    background: var(--chip-fill);
    color: var(--ink);
    font-size: var(--fs-small);
    font-weight: 500;
    white-space: nowrap;
  }

  .pings__remove {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--dim-text);
    font: inherit;
    font-size: var(--fs-body);
    line-height: 1;
    cursor: pointer;
  }

  .pings__remove:hover {
    background: var(--row-hover);
    color: var(--ink);
  }

  .pings__add {
    display: inline-flex;
  }

  .pings__addfield {
    width: 4.375rem;
    height: 32px;
    min-height: 32px;
  }

  .pings__addbtn {
    min-height: 32px;
    border-radius: 16px;
  }

  @media (max-width: 599px) {
    .pings {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
