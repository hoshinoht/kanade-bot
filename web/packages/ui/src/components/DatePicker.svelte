<!--
  The date picker (P_Dates, P_DatesSpec, P_DatesPhone): a pill trigger and a
  Thursday-first month in a popover, or a bottom sheet in the phone frame.
  `range` picks a start then an end and commits on Apply; `single` ("Since")
  commits on the first pick. Days are guild-local; today is the server's.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import MonthGrid from './MonthGrid.svelte';
  import { PHONE_QUERY } from '../media';
  import {
    dayCount,
    dayLabel,
    dayOf,
    firstOfMonth,
    isoOf,
    monthOf,
    monthTitle,
    ordered,
    parseDay,
    rangeChips,
    rangeWords,
    sinceChips,
    typedError,
    zoneWords,
    type ServerClock,
    type TypedError,
  } from './calendar';

  let {
    mode = 'range',
    label,
    clock,
    from = '',
    to = '',
    value = '',
    lead = 'Since',
    onrange,
    onpick,
  }: {
    mode?: 'range' | 'single';
    /** The trigger's overline and the sheet's title: "Dates", "Since". */
    label: string;
    clock: ServerClock | null;
    from?: string;
    to?: string;
    value?: string;
    /** Single mode's footer words before the date: "Changes since". */
    lead?: string;
    onrange?: (from: string, to: string) => void;
    onpick?: (value: string) => void;
  } = $props();

  const uid = $props.id();
  let dialog = $state<HTMLDialogElement>();
  let trigger = $state<HTMLButtonElement>();
  let open = $state(false);
  let phone = $state(false);
  // The draft: a start, an end, and whether the end is still awaited.
  let a = $state<number | null>(null);
  let b = $state<number | null>(null);
  let pending = $state(false);
  let view = $state({ y: 1970, m: 0 });
  let focus = $state(0);
  let fromText = $state<string | null>(null);
  let toText = $state<string | null>(null);
  let error = $state<TypedError | null>(null);
  let downOnBackdrop = false;

  const single = $derived(mode === 'single');
  const shown = $derived(single ? (dayOf(value) === null ? '' : dayLabel(dayOf(value)!)) : rangeWords(dayOf(from), dayOf(to)));
  const zone = $derived(clock ? zoneWords(clock.timezone) : '');
  const today = $derived(clock?.today ?? 0);

  async function show() {
    if (!clock || !dialog) return;
    const now = clock.today;
    if (single) {
      a = b = dayOf(value);
    } else {
      const f = dayOf(from);
      const t = dayOf(to);
      a = f ?? t;
      b = t ?? (f === null ? null : now);
    }
    pending = false;
    fromText = toText = null;
    error = null;
    const anchor = Math.min(now, b ?? a ?? now);
    view = monthOf(anchor);
    focus = anchor;
    phone = window.matchMedia(PHONE_QUERY).matches;
    open = true;
    await tick();
    dialog.showModal();
    place();
    dialog.querySelector<HTMLButtonElement>(`[data-day="${focus}"]`)?.focus({ preventScroll: true });
  }

  function hide() {
    if (!open) return;
    open = false;
    dialog?.close();
    trigger?.focus({ preventScroll: true });
  }

  // Under the trigger, flipped above it or held inside the viewport when short; the sheet needs none.
  function place() {
    if (!dialog || !trigger || phone) return;
    const r = trigger.getBoundingClientRect();
    const w = dialog.offsetWidth;
    const h = dialog.offsetHeight;
    let top = r.bottom + 6;
    if (top + h > innerHeight - 8) top = r.top - 6 - h >= 8 ? r.top - 6 - h : Math.max(8, innerHeight - h - 8);
    dialog.style.setProperty('--dp-left', `${Math.max(8, Math.min(r.left, innerWidth - w - 8))}px`);
    dialog.style.setProperty('--dp-top', `${top}px`);
  }

  $effect(() => {
    if (!open) return;
    const again = () => place();
    window.addEventListener('resize', again);
    return () => window.removeEventListener('resize', again);
  });

  function turnTo(day: number) {
    const next = monthOf(day);
    if (next.y !== view.y || next.m !== view.m) view = next;
  }

  function pickDay(day: number) {
    focus = day;
    if (single) {
      onpick?.(isoOf(day));
      hide();
      return;
    }
    fromText = toText = null;
    error = null;
    if (!pending || a === null) {
      a = b = day;
      pending = true;
    } else {
      [a, b] = ordered(a, day);
      pending = false;
    }
  }

  function useRange(start: number, end: number) {
    a = start;
    b = end;
    pending = false;
    fromText = toText = null;
    error = null;
    focus = end;
    turnTo(end);
  }

  function month(delta: number) {
    const n = view.y * 12 + view.m + delta;
    view = { y: Math.floor(n / 12), m: n % 12 };
    focus = Math.min(today, a !== null && monthOf(a).m === view.m && monthOf(a).y === view.y ? a : firstOfMonth(view.y, view.m));
  }

  const nextMonthStart = $derived(firstOfMonth(view.m === 11 ? view.y + 1 : view.y, (view.m + 1) % 12));

  function commitTyped() {
    if (fromText === null && toText === null) return;
    const f = fromText ?? (a === null ? '' : isoOf(a));
    const t = toText ?? (b === null || pending ? '' : isoOf(b));
    if (!f.trim() && !t.trim()) {
      a = b = null;
      pending = false;
      fromText = toText = null;
      error = null;
      return;
    }
    error = typedError(f, t, today);
    if (!error) useRange(parseDay(f)!, parseDay(t)!);
  }

  function apply() {
    if (pending || error) return;
    onrange?.(a === null ? '' : isoOf(a), b === null ? '' : isoOf(b));
    hide();
  }

  function anyDate() {
    if (single) onpick?.('');
    else onrange?.('', '');
    hide();
  }

  const rangeOn = (start: number, end: number) => !pending && a === start && b === end;
  const summary = $derived.by(() => {
    if (single) return a === null ? { main: 'any date', sub: 'every change' } : { main: `${dayLabel(a)} 00:00`, sub: zone };
    if (a === null || b === null) return { main: 'Any date', sub: `pick a start day · ${zone}` };
    if (pending) return { main: `${dayLabel(a)} → …`, sub: phone ? 'tap an end day' : 'pick an end day' };
    return { main: rangeWords(a, b), sub: phone ? dayCount(a, b) : `${dayCount(a, b)} · ${zone}` };
  });
</script>

{#snippet chips()}
  {#if clock}
    <div class="datepick__chips" data-fid="dr-quick" role="group" aria-label="Quick picks">
      {#if single}
        {#each sinceChips(clock) as chip (chip.label)}
          {@const on = a === chip.day}
          <button type="button" class="datepick__chip" class:datepick__chip--on={on} aria-pressed={on} onclick={() => pickDay(chip.day)}
            >{#if on}<Icon name="check" />{/if}{chip.label}</button
          >
        {/each}
      {:else}
        {#each rangeChips(clock) as chip (chip.label)}
          {@const on = rangeOn(chip.from, chip.to)}
          <button type="button" class="datepick__chip" class:datepick__chip--on={on} aria-pressed={on} onclick={() => useRange(chip.from, chip.to)}
            >{#if on}<Icon name="check" />{/if}{chip.label}</button
          >
        {/each}
      {/if}
    </div>
  {/if}
{/snippet}

{#snippet foot()}
  <p class="datepick__summary" data-fid="dr-summary" aria-live="polite">
    {#if single}<span class="datepick__lead">{lead}</span>{/if}<strong class="mono">{summary.main}</strong><span class="datepick__sub">{summary.sub}</span>
  </p>
  {#if single}
    {#if value}
      <div class="datepick__actions"><button type="button" class="datepick__link" onclick={anyDate}>Any date</button></div>
    {/if}
  {:else if phone}
    <div class="datepick__actions">
      <button type="button" class="datepick__btn" onclick={anyDate}>Any date</button>
      <button type="button" class="datepick__key" data-fid="dr-apply" disabled={pending} onclick={apply}>Apply</button>
    </div>
  {:else}
    <div class="datepick__actions">
      <button type="button" class="datepick__link" onclick={anyDate}>Any date</button>
      <span class="datepick__spacer"></span>
      <button type="button" class="datepick__btn" onclick={hide}>Cancel</button>
      <button type="button" class="datepick__key" data-fid="dr-apply" disabled={pending || Boolean(error)} onclick={apply}>Apply</button>
    </div>
  {/if}
{/snippet}

{#snippet body(clock: ServerClock)}
  {#if phone}
    <span class="datepick__handle" aria-hidden="true"></span>
    <div class="datepick__title" data-fid="dr-title">
      <h2 id="{uid}-title">{label}</h2>
      <button type="button" class="datepick__icon" onclick={hide}><Icon name="x" label="Close" /></button>
    </div>
  {:else if !single}
    <div class="datepick__fields" data-fid="dr-fields">
      <label class="datepick__field" class:datepick__field--bad={error?.field === 'from'}
        ><span class="cap">From</span><input
          class="mono"
          aria-label="From date"
          placeholder="YYYY-MM-DD"
          inputmode="numeric"
          autocomplete="off"
          aria-invalid={error?.field === 'from' || undefined}
          aria-describedby={error ? `${uid}-error` : undefined}
          value={fromText ?? (a === null ? '' : isoOf(a))}
          oninput={(event) => {
            fromText = event.currentTarget.value;
            error = null;
          }}
          onchange={commitTyped}
          onkeydown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              commitTyped();
            }
          }}
        /></label
      >
      <span class="datepick__arrow" aria-hidden="true">→</span>
      <label class="datepick__field" class:datepick__field--bad={error?.field === 'to'}
        ><span class="cap">To</span><input
          class="mono"
          aria-label="To date"
          placeholder="YYYY-MM-DD"
          inputmode="numeric"
          autocomplete="off"
          aria-invalid={error?.field === 'to' || undefined}
          aria-describedby={error ? `${uid}-error` : undefined}
          value={toText ?? (b === null || pending ? '' : isoOf(b))}
          oninput={(event) => {
            toText = event.currentTarget.value;
            error = null;
          }}
          onchange={commitTyped}
          onkeydown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              commitTyped();
            }
          }}
        /></label
      >
    </div>
    {#if error}
      <p class="datepick__error" id="{uid}-error" data-fid="dr-error" role="alert"><Icon name="alert-circle" /><span>{error.message}</span></p>
    {/if}
  {/if}
  {@render chips()}
  <div class="datepick__month" data-fid="dr-month">
    <button type="button" class="datepick__icon" onclick={() => month(-1)}><Icon name="chevron-left" label="Previous month" /></button>
    <span class="datepick__month-title" aria-live="polite">{monthTitle(view.y, view.m)}</span>
    <button type="button" class="datepick__icon" disabled={nextMonthStart > today} onclick={() => month(1)}><Icon name="chevron-right" label="Next month" /></button>
  </div>
  <MonthGrid
    y={view.y}
    m={view.m}
    firstDow={clock.firstDow}
    {today}
    from={a}
    to={b}
    {focus}
    onpick={pickDay}
    onmove={(day) => {
      focus = day;
      turnTo(day);
    }}
  />
  <!-- The phone sheet has no footer box: the summary carries the rule (P_DatesPhone). -->
  {#if phone}
    {@render foot()}
  {:else}
    <div class="datepick__foot" data-fid="dr-foot">{@render foot()}</div>
  {/if}
{/snippet}

<button
  bind:this={trigger}
  type="button"
  class="datepick-trigger"
  class:datepick-trigger--set={shown}
  class:datepick-trigger--open={open}
  data-fid="dr-trigger"
  aria-haspopup="dialog"
  aria-expanded={open}
  aria-controls="{uid}-dialog"
  disabled={!clock}
  onclick={() => void show()}
>
  <Icon name="calendar" /><span class="datepick-trigger__label">{label}</span><span class="datepick-trigger__value" class:datepick-trigger__value--empty={!shown}
    >{shown || 'any date'}</span
  ><span class="datepick-trigger__chev"><Icon name="chevron-down" /></span>
</button>

<!-- Escape is handled here (and stopped) so a filters popover around the trigger stays open. -->
<dialog
  bind:this={dialog}
  id="{uid}-dialog"
  class="datepick"
  class:datepick--sheet={phone}
  class:datepick--single={single}
  aria-label={phone ? undefined : single ? label : 'Date range'}
  aria-labelledby={phone ? `${uid}-title` : undefined}
  onkeydown={(event) => {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    event.stopPropagation();
    hide();
  }}
  oncancel={(event) => {
    event.preventDefault();
    hide();
  }}
  onpointerdown={(event) => (downOnBackdrop = event.target === dialog)}
  onclick={(event) => {
    const dismiss = downOnBackdrop && event.target === dialog;
    downOnBackdrop = false;
    if (dismiss) hide();
  }}
>
  {#if open && clock}
    {#if phone}
      <div class="datepick__panel" data-fid="dr-sheet">{@render body(clock)}</div>
    {:else}
      <div class="datepick__panel" data-fid="dr-pop">{@render body(clock)}</div>
    {/if}
  {/if}
</dialog>
