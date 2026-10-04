<!--
  The Reminders window's "Filters (n)" title-bar button (B_Reminders): the old
  filter row (kind, run, member, day) in a popover under it, filtering as you
  choose. Escape or a press outside folds it away.
-->
<script module lang="ts">
  export interface ReminderFilter {
    kind: string;
    run: string;
    member: string;
    day: string;
  }
  export const NO_FILTER: ReminderFilter = { kind: '', run: '', member: '', day: '' };
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { Icon, Select, type SelectOption } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';

  let {
    filter = $bindable(),
    kinds,
    runs,
    people,
    days,
  }: { filter: ReminderFilter; kinds: string[]; runs: { id: string; label: string }[]; people: string[]; days: string[] } = $props();
  const uid = $props.id();
  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();
  const count = $derived(Object.values(filter).filter(Boolean).length);
  const kindOptions = $derived<SelectOption[]>([{ value: '', label: 'every kind' }, ...kinds.map((k) => ({ value: k, label: k }))]);
  const runOptions = $derived<SelectOption[]>([{ value: '', label: 'every run' }, ...runs.map((r) => ({ value: r.id, label: r.label }))]);
  const memberOptions = $derived<SelectOption[]>([{ value: '', label: 'anyone', icon: 'users' }, ...people.map((p) => ({ value: p, label: p, mono: p.slice(0, 1).toUpperCase() }))]);
  const dayOptions = $derived<SelectOption[]>([{ value: '', label: 'every day', icon: 'calendar' }, ...days.map((d) => ({ value: d, label: d }))]);

  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    panel?.querySelector<HTMLElement>('button.dd, select')?.focus({ preventScroll: true });
  }

  function close(refocus: boolean) {
    open = false;
    if (refocus) button?.focus({ preventScroll: true });
  }

  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel?.contains(target) && !button?.contains(target)) close(false);
    };
    document.addEventListener('pointerdown', away, true);
    return () => document.removeEventListener('pointerdown', away, true);
  });
</script>

<div class="reminders-filters">
  <button
    type="button"
    class="btn reminders-filters__toggle"
    data-fid="reminders-filters"
    aria-expanded={open}
    aria-controls="{uid}-panel"
    bind:this={button}
    onclick={() => void toggle()}><Icon name="filter" /><span>Filters ({count})</span></button
  >
  {#if open}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      class="filters filters--panel reminders-filters__panel"
      role="group"
      aria-label="Filter reminders"
      id="{uid}-panel"
      bind:this={panel}
      onkeydown={(event) => {
        if (event.key === 'Escape') {
          event.stopPropagation();
          close(true);
        }
      }}
    >
      <Select size="bar" label="Kind" options={kindOptions} bind:value={filter.kind} noun="kinds" />
      <Select size="bar" label="Run" options={runOptions} bind:value={filter.run} noun="runs" />
      <Select size="bar" label="Member" options={memberOptions} bind:value={filter.member} noun="members" />
      <Select size="bar" label="Day" options={dayOptions} bind:value={filter.day} noun="days" />
      {#if count}<button class="btn btn--ghost" type="button" onclick={() => (filter = { ...NO_FILTER })}>Clear</button>{/if}
    </div>
  {/if}
</div>
