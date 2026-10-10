<!--
  The dropdown (P_Select, P_SelectSpec, P_SelectPhone): a select-only combobox
  whose focus stays on the pill trigger, with a rounded listbox popover in the
  top layer. `size`: "bar" (36 px, label + value, tonal when filtering),
  "field" (40 px, full width; the label sits outside) or "tbar" (32 px on a
  window's title bar). Below 840 px or on a coarse pointer the pill wraps a
  real, transparent <select>, so a tap opens the phone's own picker.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';
  import SelectList from './SelectList.svelte';
  import { follow, placePopover, pressAway, watchMedia } from './dropdown';
  import { filterOptions, groupsOf, hasSearch, NATIVE_QUERY, navigate, startRow, typeAhead, type SelectOption } from './select';

  let {
    value = $bindable(''),
    options,
    label,
    fullLabel,
    size = 'field',
    placeholder = '',
    neutral = '',
    plain = false,
    disabled = false,
    invalid = false,
    error = '',
    describedby,
    name,
    noun = 'options',
    align = size === 'tbar' ? 'end' : 'start',
    class: extra = '',
    onchange,
  }: {
    value?: string;
    options: SelectOption[];
    /** The accessible name, also the pill's overline in "bar" and "tbar". */
    label: string;
    /** A longer accessible name that starts with `label` ("Sort" → "Sort members"). */
    fullLabel?: string;
    size?: 'bar' | 'field' | 'tbar';
    /** Shown dim when no option has the value (a required pick, or a disabled field's reason). */
    placeholder?: string;
    /** The no-filter value: any other value draws the pill tonal ("bar", "tbar"). */
    neutral?: string;
    /** Never tonal: a choice such as sort order, not a filter. */
    plain?: boolean;
    disabled?: boolean;
    invalid?: boolean;
    /** Words under the field: with `invalid`, a 2 px risk ring plus these. */
    error?: string;
    describedby?: string;
    /** Submitted with a surrounding form. */
    name?: string;
    /** What the search box counts: "Search 31 people". */
    noun?: string;
    align?: 'start' | 'end';
    class?: string;
    onchange?: (value: string) => void;
  } = $props();

  const uid = $props.id();
  let native = $state(false);
  let open = $state(false);
  let active = $state<string | null>(null);
  let query = $state('');
  let trigger = $state<HTMLButtonElement>();
  let select = $state<HTMLSelectElement>();
  let pop = $state<HTMLDivElement>();
  let search = $state<HTMLInputElement>();
  let typed = '';
  let typedAt = 0;

  $effect(() => watchMedia(NATIVE_QUERY, (on) => (native = on)));

  const current = $derived(options.find((o) => o.value === value));
  const shown = $derived(current?.label ?? placeholder);
  const tonal = $derived(!plain && size !== 'field' && value !== neutral && Boolean(current));
  const accessibleName = $derived(fullLabel ?? label);
  const searchable = $derived(hasSearch(options));
  const visible = $derived(searchable && query ? filterOptions(options, query) : options);
  const index = $derived(new Map(options.map((o, i) => [o.value, i])));
  const idOf = (v: string) => `${uid}-o${index.get(v) ?? 'x'}`;
  const errorId = $derived(error ? `${uid}-error` : undefined);
  const described = $derived([describedby, errorId].filter(Boolean).join(' ') || undefined);

  /** Focuses the trigger (or the native select on phones). */
  export function focus() {
    (native ? select : trigger)?.focus({ preventScroll: true });
  }

  async function show() {
    if (disabled || open || !trigger) return;
    query = '';
    active = startRow(options, value);
    open = true;
    // The popover exists only while open, so closed dropdowns add nothing to the page.
    await tick();
    if (!pop) return;
    pop.showPopover();
    placePopover(trigger, pop, align);
    scrollActive();
    if (searchable) search?.focus({ preventScroll: true });
  }

  function hide(refocus = true) {
    if (!open) return;
    open = false;
    query = '';
    if (pop?.matches(':popover-open')) pop.hidePopover();
    if (refocus && document.activeElement !== trigger) trigger?.focus({ preventScroll: true });
  }

  function pick(next: string) {
    hide();
    if (next === value) return;
    value = next;
    onchange?.(next);
  }

  async function scrollActive() {
    await tick();
    if (active !== null) document.getElementById(idOf(active))?.scrollIntoView({ block: 'nearest' });
  }

  $effect(() => {
    if (!open) return;
    const stopAway = pressAway(() => [trigger, pop], () => hide(false));
    const stopFollow = follow(() => pop, () => trigger && pop && placePopover(trigger, pop, align));
    return () => {
      stopAway();
      stopFollow();
    };
  });

  function key(event: KeyboardEvent, fromSearch: boolean) {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const k = event.key;
    if (!open) {
      if (k === 'ArrowDown' || k === 'ArrowUp' || k === 'Enter' || k === ' ') {
        event.preventDefault();
        void show();
      } else if (k.length === 1 && /\S/.test(k) && !searchable) {
        // Type-ahead while closed opens on the match.
        void show().then(() => ahead(k));
      }
      return;
    }
    const moved = navigate(k, visible, active);
    if (moved !== undefined) {
      event.preventDefault();
      active = moved;
      void scrollActive();
      return;
    }
    if (k === 'Enter' || (k === ' ' && !fromSearch && !typing())) {
      event.preventDefault();
      if (active !== null) pick(active);
    } else if (k === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      hide();
    } else if (!fromSearch && k.length === 1 && (/\S/.test(k) || typing())) {
      event.preventDefault();
      ahead(k);
    }
  }

  const typing = () => typed !== '' && performance.now() - typedAt < 600;

  function ahead(k: string) {
    typed = (typing() ? typed : '') + k;
    typedAt = performance.now();
    const hit = typeAhead(visible, active, typed);
    if (hit !== null) {
      active = hit;
      void scrollActive();
    }
  }

  function onQuery(next: string) {
    query = next;
    active = startRow(filterOptions(options, next), value);
    void scrollActive();
  }
</script>

{#snippet face()}
  {#if size !== 'field'}<span class="dd__label">{label}</span>{/if}<span class="dd__value">{shown}</span><span class="dd__chev" aria-hidden="true"
    ><Icon name="chevron-down" /></span
  >
{/snippet}

<div class="ddw ddw--{size}" class:ddw--native={native}>
  {#if native}
    <!-- Phones: the pill is drawn; the transparent native select over it takes the tap. -->
    <span
      class="dd dd--{size} {extra}"
      class:dd--set={tonal}
      class:dd--empty={!current}
      class:dd--bad={invalid}
      class:dd--off={disabled}
      data-fid="dd-trigger"
    >
      {@render face()}
      <select
        bind:this={select}
        class="dd__native"
        aria-label={accessibleName}
        {name}
        {disabled}
        aria-invalid={invalid || undefined}
        aria-describedby={described}
        {value}
        onchange={(event) => {
          value = event.currentTarget.value;
          onchange?.(value);
        }}
      >
        {#if !current}<option value={value} disabled>{placeholder || ' '}</option>{/if}
        {#each groupsOf(options) as group, g (g)}
          {#if group.label}
            <optgroup label={group.label}>
              {#each group.options as o (o.value)}<option value={o.value} disabled={o.disabled}>{o.label}</option>{/each}
            </optgroup>
          {:else}
            {#each group.options as o (o.value)}<option value={o.value} disabled={o.disabled}>{o.label}</option>{/each}
          {/if}
        {/each}
      </select>
    </span>
  {:else}
    <button
      bind:this={trigger}
      type="button"
      role="combobox"
      class="dd dd--{size} {extra}"
      class:dd--set={tonal}
      class:dd--empty={!current}
      class:dd--open={open}
      class:dd--bad={invalid}
      data-fid="dd-trigger"
      data-value={value}
      aria-label={accessibleName}
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-controls="{uid}-list"
      aria-activedescendant={open && !searchable && active !== null ? idOf(active) : undefined}
      aria-invalid={invalid || undefined}
      aria-describedby={described}
      {disabled}
      onclick={() => (open ? hide() : void show())}
      onkeydown={(event) => key(event, false)}
      onblur={(event) => {
        // Leaving for anything outside the popover (Tab, a press elsewhere) closes it.
        if (open && !pop?.contains(event.relatedTarget as Node)) hide(false);
      }}
    >
      {@render face()}
    </button>
    {#if name}<input type="hidden" {name} {value} />{/if}
  {/if}
  {#if error}<p class="dd-error" id={errorId}><Icon name="alert-circle" /><span>{error}</span></p>{/if}
  {#if !native && open}
    <!-- A press inside keeps focus where it is (trigger or search box). -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      bind:this={pop}
      popover="manual"
      class="dd-pop dd-pop--{size}"
      data-fid="dd-pop"
      onmousedown={(event) => {
        if (event.target !== search) event.preventDefault();
      }}
    >
      {#if searchable}
        <label class="dd-search" data-fid="dd-search"
          ><Icon name="search" /><input
            bind:this={search}
            type="text"
            role="combobox"
            autocomplete="off"
            spellcheck="false"
            aria-label="Filter {noun}"
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls="{uid}-list"
            aria-activedescendant={active !== null ? idOf(active) : undefined}
            placeholder="Search {options.length} {noun}"
            value={query}
            oninput={(event) => onQuery(event.currentTarget.value)}
            onkeydown={(event) => key(event, true)}
            onblur={(event) => {
              if (open && event.relatedTarget !== trigger && !pop?.contains(event.relatedTarget as Node)) hide(false);
            }}
          /></label
        >
      {/if}
      <SelectList id="{uid}-list" label={accessibleName} options={visible} {active} isOn={(v) => v === value} {idOf} onpick={pick} query={searchable ? query : ''} />
      {#if visible.length === 0}
        <p class="dd-empty" role="status">
          <b>Nothing matches “{query}”.</b>
          <button type="button" class="dd-link" onclick={() => onQuery('')}>Clear search</button>
        </p>
      {/if}
      {#if size !== 'tbar' || searchable}
        <div class="dd-foot" data-fid="dd-foot" aria-hidden="true">
          <span><span class="mono">↑↓</span> move · <span class="mono">↵</span> pick · <span class="mono">esc</span> close</span>
          {#if searchable}<span class="mono">{visible.length} of {options.length}</span>{/if}
        </div>
      {/if}
    </div>
  {/if}
</div>
