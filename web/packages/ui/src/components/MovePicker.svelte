<!--
  The Move picker (admin boards Main, P_MoveWidths, P_MoveStates,
  P_MovePhone; the member portal's run pane and Move page): the boss week's
  day strip, the time stepper (Config → Run lengths), the typed shortcut read
  live ("reads as …", errors only on Enter or blur), suggestions from the
  picked day's runs and the planner's clash wording, which warns and never
  blocks. `foot` adds "Moves to …", Cancel and Move. Styles come with the
  caller (`@kanade/ui/styles/move-picker.scss`); fidelity tags too (`fids`).
-->
<script lang="ts" module>
  /** `data-fid` tags for the picker's regions, from the app's call site. */
  export interface MovePickerFids {
    picker?: string;
    time?: string;
    type?: string;
    error?: string;
    suggest?: string;
    clash?: string;
    acts?: string;
    result?: string;
    submit?: string;
  }
</script>

<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import type { WeekDay } from '@kanade/api-types';
  import { clashText, dayCells, edgeDay, readTyped, suggestions, type PickerRun } from '../move/picker';
  import { fromMinutes, toMinutes, type Slot } from '../move/slot';
  import DayStrip from './DayStrip.svelte';
  import Icon from './Icon.svelte';
  import TimeStepper from './TimeStepper.svelte';

  let {
    days,
    subject,
    own,
    initial,
    field,
    names,
    step,
    variant = 'pane',
    legend = 'Move to',
    aside = '',
    clashTail = 'You can still move; they will be double-booked.',
    busy = false,
    foot = false,
    fill = false,
    autofocus = false,
    value = $bindable(),
    blocked = $bindable(true),
    hint = '',
    submitLabel = 'Move',
    fids = {},
    showClash = true,
    extra,
    onsubmit,
    oncancel,
  }: {
    /** The boss week the run is in; dates may be empty (weekday names only). */
    days: WeekDay[];
    /** The run being moved: id, title, length and who plays. */
    subject: PickerRun;
    /** Where it is now: the dashed "from" day and the unchanged slot; null for a new run. */
    own: Slot | null;
    /** Where the pick starts (the proposed slot in the Inbox); `own` when absent. */
    initial?: Slot;
    /** The week's live runs: dots, suggestions and clashes. */
    field: PickerRun[];
    names: (id: string) => string;
    step: number;
    /** pane: run pane and laptop sheet; phone: the phone sheet; card: the Inbox decision card. */
    variant?: 'pane' | 'phone' | 'card';
    legend?: string;
    /** The legend's right side ("from Tue 06 · 23:30"). */
    aside?: string;
    clashTail?: string;
    busy?: boolean;
    foot?: boolean;
    /** Fill a view's height: the choices scroll, the foot stays at the bottom. */
    fill?: boolean;
    /** Focus the picked day on mount (the sheet's Move view). */
    autofocus?: boolean;
    /** The slot it reads now (out). */
    value?: Slot;
    /** True while there is nothing to move to or a typed error stands (out). */
    blocked?: boolean;
    /** Replaces the stepper hint ("↑ ↓ step 30 min · Run lengths"). */
    hint?: string;
    submitLabel?: string;
    fids?: MovePickerFids;
    /** The clash line under the suggestions; off where the caller lists clashes itself (`extra`). */
    showClash?: boolean;
    /** Below the suggestions: what the caller checks about the picked slot (the member's checklist). */
    extra?: Snippet<[Slot]>;
    onsubmit: (slot: Slot) => void;
    /** Cancel and Escape: leave the picker (the sheet's Move view); without it they only reset. */
    oncancel?: () => void;
  } = $props();

  const uid = $props.id();
  const start = (): Slot => initial ?? own ?? { day: days.find((d) => d.is_today)?.index ?? 0, time: null };
  let picked = $state<Slot>(start());
  let typed = $state('');
  let checked = $state(false);
  let root = $state<HTMLElement>();

  const counts = $derived(days.map((d) => field.filter((r) => r.day === d.index && r.id !== subject.id).length));
  const cells = $derived(dayCells(days, picked.day, own?.day ?? null, (day) => counts[day] ?? 0));
  const reading = $derived(typed.trim() ? readTyped(typed, days, cells, picked) : null);
  const slot = $derived(reading?.kind === 'ok' ? reading.slot : picked);
  const shown = $derived(dayCells(days, slot.day, own?.day ?? null, (day) => counts[day] ?? 0));
  const error = $derived(reading?.kind === 'error' && checked ? reading.message : '');
  const same = $derived(own !== null && slot.day === own.day && slot.time === own.time);
  const past = $derived(Boolean(shown[slot.day]?.past));
  const ownTime = $derived(own !== null && own.time === null);
  const ideas = $derived(suggestions(subject, own ?? { day: -1, time: null }, slot.day, field, step));
  const clash = $derived(clashText(subject, slot, field, names));
  const label = (s: Slot) => {
    const cell = shown[s.day];
    return `${cell ? (cell.date ? `${cell.dow} ${cell.date}` : cell.dow) : `day ${s.day + 1}`} ${s.time ?? 'own time'}`;
  };

  $effect.pre(() => {
    value = slot;
    blocked = Boolean(error) || same || past;
  });

  onMount(() => {
    if (autofocus) root?.querySelector<HTMLElement>('.daystrip__day[aria-checked="true"]')?.focus({ preventScroll: true });
  });

  /** An explicit control: the live typed reading becomes the pick, and the field clears. */
  function set(change: Partial<Slot>) {
    picked = { ...slot, ...change };
    typed = '';
    checked = false;
  }

  function reset() {
    picked = start();
    typed = '';
    checked = false;
  }

  function submit() {
    if (typed.trim() && reading?.kind === 'error') {
      checked = true;
      return;
    }
    if (busy || same || past) return;
    onsubmit(slot);
  }

  function typedKey(event: KeyboardEvent) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    if (reading?.kind === 'ok') set(reading.slot);
    else if (reading) checked = true;
  }

  function escape(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    if (typed) {
      typed = '';
      checked = false;
    } else if (oncancel) oncancel();
    else if (picked.day !== start().day || picked.time !== start().time) reset();
    else return;
    event.preventDefault();
  }

  function setTime() {
    const first = ideas.find((s) => s.label !== 'same time');
    set({ time: first?.time ?? '21:00' });
  }
</script>

<div class="movepick movepick--{variant}" class:movepick--fill={fill} bind:this={root} onkeydown={escape} role="presentation">
  <fieldset class="movepick__set" data-fid={fids.picker}>
    <legend class="movepick__legend">
      <span class="cap movepick__title">{legend}</span>
      {#if aside}<span class="mono movepick__from">{aside}</span>{/if}
    </legend>
    <DayStrip
      days={shown.map((c) => ({ value: c.index, dow: c.dow, date: c.date, tag: c.tag, dots: c.dots, past: c.past, today: c.today, reset: c.reset, from: c.from, label: c.label }))}
      value={slot.day}
      label="Day"
      home={edgeDay(shown, 'home')}
      onpick={(day) => set({ day })}
      onenter={submit}
    />
    {#if variant === 'phone'}<span class="cap movepick__title">Time</span>{/if}
    <div class="movepick__row" data-fid={fids.time}>
      <TimeStepper
        value={slot.time === null ? null : toMinutes(slot.time)}
        {step}
        invalid={Boolean(error)}
        onchange={(minutes) => set({ time: fromMinutes(minutes) })}
        onenter={submit}
      />
      {#if ownTime}
        <button type="button" class="btn movepick__own" onclick={() => (slot.time === null ? setTime() : set({ time: null }))}
          >{slot.time === null ? 'Set a time' : 'Keep own time'}</button
        >
      {/if}
      {#if variant === 'pane'}<span class="movepick__or" aria-hidden="true">or</span>{/if}
      <label class="movepick__typed" class:movepick__typed--bad={error} data-fid={fids.type}>
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
          ><rect x="2" y="6" width="20" height="12" rx="2" /><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" /></svg
        >
        <input
          class="mono"
          bind:value={typed}
          oninput={() => (checked = false)}
          onkeydown={typedKey}
          onblur={() => (checked = Boolean(typed.trim()))}
          aria-label={variant === 'pane' ? 'Type a day and time' : 'Or type a day and time'}
          aria-invalid={error ? 'true' : undefined}
          aria-describedby={error ? `${uid}-err` : `${uid}-hint`}
          placeholder={variant === 'card' ? 'or type' : variant === 'phone' ? 'or type: wed 21:30, 9:45pm' : 'wed 21:30, 9:45pm'}
          autocomplete="off"
          spellcheck="false"
        />
      </label>
    </div>
    <p class="movepick__hint" id="{uid}-hint">
      {#if variant !== 'card'}<span>{hint || (variant === 'phone' ? `steps ${step} min` : `↑ ↓ step ${step} min · Run lengths`)}</span>{/if}
      <span class="mono movepick__reads" aria-live="polite">{reading?.kind === 'ok' ? `reads as ${label(reading.slot)}` : ''}</span>
    </p>
    {#if error}
      <p class="movepick__error" id="{uid}-err" role="alert" data-fid={fids.error}><Icon name="alert-circle" />{error}</p>
    {/if}
    {#if ideas.length}
      {#if variant !== 'card'}<span class="cap movepick__title movepick__title--sub">Suggestions · {label({ day: slot.day, time: null }).replace(/ own time$/, '')}</span>{/if}
      <div class="movepick__ideas" role="group" aria-label="Suggestions" data-fid={fids.suggest}>
        {#each ideas as idea (idea.label)}
          {@const hit = clashText(subject, { day: slot.day, time: idea.time }, field, names)}
          {@const on = idea.time === slot.time}
          <button
            type="button"
            class="movepick__idea"
            class:movepick__idea--on={on}
            class:movepick__idea--clash={hit}
            aria-pressed={on}
            aria-label="{idea.label} {idea.time}{hit ? `, clash: ${hit}` : ''}"
            onclick={() => set({ time: idea.time })}
            >{#if on}<Icon name="check" />{:else if hit}<Icon name="alert-triangle" />{/if}{idea.label} · <b class="mono">{idea.time}</b></button
          >
        {/each}
      </div>
    {/if}
    {#if clash && showClash}
      <p class="movepick__clash" role="status" data-fid={fids.clash}><Icon name="alert-triangle" /><span><b>Clash:</b> {clash}. {clashTail}</span></p>
    {/if}
    {@render extra?.(slot)}
  </fieldset>
  {#if foot}
    <div class="movepick__foot">
      <p class="movepick__result">
        <span class="cap">Moves to</span>
        <b class="mono movepick__to" data-fid={fids.result}>{label(slot)}</b>
        {#if own && !same}<s class="mono movepick__was"><span class="vh">was </span>{label(own)}</s>{/if}
      </p>
      <div class="movepick__acts" data-fid={fids.acts}>
        <button type="button" class="btn" disabled={busy} onclick={() => (oncancel ? oncancel() : reset())}>Cancel</button>
        <button type="button" class="btn btn--primary btn--key movepick__go" data-fid={fids.submit} disabled={busy || blocked} onclick={submit}
          ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
            ><path d="M5 12h14M13 6l6 6-6 6" /></svg
          >{submitLabel}</button
        >
      </div>
    </div>
  {/if}
</div>
