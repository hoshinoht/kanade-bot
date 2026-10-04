<!--
  The multi-select dropdown (P_SelectSpec "Re-read channels"): the trigger
  counts ("3 of 8", or names the one picked); the popover has All · None, rows
  that stay open after a toggle (Space), and Done (Enter closes). Below 840 px
  or on a coarse pointer it opens a full-screen sheet of checkboxes instead.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import SelectList from './SelectList.svelte';
  import { follow, placePopover, pressAway, watchMedia } from './dropdown';
  import { allOf, countWords, NATIVE_QUERY, navigate, startRow, toggle, typeAhead, type SelectOption } from './select';

  let {
    values = $bindable([]),
    options,
    label,
    fullLabel,
    size = 'bar',
    noun = 'options',
    disabled = false,
    onchange,
  }: {
    values?: string[];
    options: SelectOption[];
    /** The accessible name and, in "bar", the pill's overline. */
    label: string;
    /** A longer accessible name that starts with `label`. */
    fullLabel?: string;
    size?: 'bar' | 'field';
    /** The head's count: "8 watched channels". */
    noun?: string;
    disabled?: boolean;
    onchange?: (values: string[]) => void;
  } = $props();

  const uid = $props.id();
  let sheet = $state(false);
  let open = $state(false);
  let active = $state<string | null>(null);
  let trigger = $state<HTMLButtonElement>();
  let pop = $state<HTMLDivElement>();
  let dialog = $state<HTMLDialogElement>();
  let typed = '';
  let typedAt = 0;

  $effect(() => watchMedia(NATIVE_QUERY, (on) => (sheet = on)));

  const words = $derived(countWords(options, values));
  const accessibleName = $derived(fullLabel ?? label);
  const index = $derived(new Map(options.map((o, i) => [o.value, i])));
  const idOf = (v: string) => `${uid}-o${index.get(v) ?? 'x'}`;

  function set(next: string[]) {
    values = next;
    onchange?.(next);
  }

  async function show() {
    if (disabled || open) return;
    open = true;
    active = startRow(options, values[0] ?? '');
    await tick();
    if (sheet) {
      dialog?.showModal();
      dialog?.querySelector<HTMLInputElement>('input')?.focus({ preventScroll: true });
      return;
    }
    if (!pop || !trigger) return;
    pop.showPopover();
    placePopover(trigger, pop, 'start');
  }

  function hide(refocus = true) {
    if (!open) return;
    open = false;
    if (pop?.matches(':popover-open')) pop.hidePopover();
    if (dialog?.open) dialog.close();
    if (refocus) trigger?.focus({ preventScroll: true });
  }

  $effect(() => {
    if (!open || sheet) return;
    const stopAway = pressAway(() => [trigger, pop], () => hide(false));
    const stopFollow = follow(() => pop, () => trigger && pop && placePopover(trigger, pop, 'start'));
    return () => {
      stopAway();
      stopFollow();
    };
  });

  async function scrollActive() {
    await tick();
    if (active !== null) document.getElementById(idOf(active))?.scrollIntoView({ block: 'nearest' });
  }

  function key(event: KeyboardEvent) {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const k = event.key;
    if (!open) {
      if (k === 'ArrowDown' || k === 'ArrowUp' || k === 'Enter' || k === ' ') {
        event.preventDefault();
        void show();
      }
      return;
    }
    const moved = navigate(k, options, active);
    if (moved !== undefined) {
      event.preventDefault();
      active = moved;
      void scrollActive();
    } else if (k === ' ' && !(typed && performance.now() - typedAt < 600)) {
      event.preventDefault();
      if (active !== null) set(toggle(options, values, active));
    } else if (k === 'Enter' || k === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      hide();
    } else if (k.length === 1) {
      event.preventDefault();
      typed = (performance.now() - typedAt < 600 ? typed : '') + k;
      typedAt = performance.now();
      const hit = typeAhead(options, active, typed);
      if (hit !== null) {
        active = hit;
        void scrollActive();
      }
    }
  }

  // Tab moves on into All, None and Done; leaving the trigger and popover closes it.
  function leaving(event: FocusEvent) {
    const to = event.relatedTarget as Node | null;
    if (open && !sheet && to !== trigger && !pop?.contains(to)) hide(false);
  }
</script>

{#snippet picks()}
  {@const all = values.length === allOf(options).length}
  <!-- aria-disabled, not disabled: a pressed link keeps focus, so the popover stays open. -->
  <span class="dd-picks">
    <button type="button" class="dd-link" aria-disabled={all} onclick={() => !all && set(allOf(options))}>All</button>
    ·
    <button type="button" class="dd-link" aria-disabled={values.length === 0} onclick={() => values.length && set([])}>None</button>
  </span>
{/snippet}

<div class="ddw ddw--{size}" class:ddw--native={sheet}>
  <button
    bind:this={trigger}
    type="button"
    role={sheet ? undefined : 'combobox'}
    class="dd dd--{size}"
    class:dd--set={values.length > 0}
    class:dd--open={open}
    data-fid="ddm-trigger"
    aria-label={accessibleName}
    aria-haspopup={sheet ? 'dialog' : 'listbox'}
    aria-expanded={open}
    aria-controls={sheet ? `${uid}-sheet` : `${uid}-list`}
    aria-activedescendant={open && !sheet && active !== null ? idOf(active) : undefined}
    {disabled}
    onclick={() => (open ? hide() : void show())}
    onkeydown={key}
    onblur={leaving}
  >
    {#if size !== 'field'}<span class="dd__label">{label}</span>{/if}<span class="dd__value">{words}</span><span class="dd__chev" aria-hidden="true"><Icon name="chevron-down" /></span>
  </button>

  {#if !sheet}
    {#if open}
      <!-- A press on a row keeps focus on the trigger; the buttons take it. -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        bind:this={pop}
        popover="manual"
        class="dd-pop dd-pop--multi"
        data-fid="ddm-pop"
        onmousedown={(event) => {
          if (!(event.target as HTMLElement).closest('button')) event.preventDefault();
        }}
        onfocusout={leaving}
        onkeydown={(event) => {
          if (event.key !== 'Escape') return;
          event.preventDefault();
          event.stopPropagation();
          hide();
        }}
      >
        <div class="dd-head" data-fid="ddm-head"><span>{options.length} {noun}</span>{@render picks()}</div>
        <SelectList id="{uid}-list" label={accessibleName} {options} {active} multi isOn={(v) => values.includes(v)} {idOf} onpick={(v) => set(toggle(options, values, v))} />
        <div class="dd-foot dd-foot--multi" data-fid="ddm-foot">
          <span><b class="mono">{values.length}</b> selected</span>
          <button type="button" class="dd-done" onclick={() => hide()}>Done</button>
        </div>
      </div>
    {/if}
  {:else}
    <dialog
      bind:this={dialog}
      id="{uid}-sheet"
      class="dd-sheet"
      aria-labelledby="{uid}-title"
      oncancel={(event) => {
        event.preventDefault();
        hide();
      }}
    >
      {#if open}
        <div class="dd-sheet__head">
          <h2 id="{uid}-title">{label}</h2>
          <button type="button" class="dd-sheet__close" onclick={() => hide()}><Icon name="x" label="Close" /></button>
        </div>
        <div class="dd-head"><span>{options.length} {noun} · {values.length} selected</span>{@render picks()}</div>
        <fieldset class="dd-sheet__list">
          <legend class="vh">{label}</legend>
          {#each options as o (o.value)}
            <label class="dd-check" class:dd-check--dis={o.disabled}
              ><input type="checkbox" checked={values.includes(o.value)} disabled={o.disabled} onchange={() => set(toggle(options, values, o.value))} /><span
                class="dd-box"
                aria-hidden="true">{#if values.includes(o.value)}<Icon name="check" />{/if}</span
              ><span class="dd-opt__text"><span class="dd-opt__label">{o.label}</span>{#if o.sub}<span class="dd-opt__sub">{o.sub}</span>{/if}</span></label
            >
          {/each}
        </fieldset>
        <button type="button" class="dd-done dd-done--sheet" onclick={() => hide()}>Done · {words}</button>
      {/if}
    </dialog>
  {/if}
</div>
