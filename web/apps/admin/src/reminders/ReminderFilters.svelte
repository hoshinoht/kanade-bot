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
  import { Icon } from '@kanade/ui';

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

  async function toggle() {
    open = !open;
    if (!open) return;
    await tick();
    panel?.querySelector<HTMLElement>('select')?.focus({ preventScroll: true });
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
      <label class="field"><span>Kind</span>
        <select bind:value={filter.kind}><option value="">every kind</option>{#each kinds as k (k)}<option value={k}>{k}</option>{/each}</select>
      </label>
      <label class="field"><span>Run</span>
        <select bind:value={filter.run}><option value="">every run</option>{#each runs as r (r.id)}<option value={r.id}>{r.label}</option>{/each}</select>
      </label>
      <label class="field"><span>Member</span>
        <select bind:value={filter.member}><option value="">anyone</option>{#each people as p (p)}<option value={p}>{p}</option>{/each}</select>
      </label>
      <label class="field"><span>Day</span>
        <select bind:value={filter.day}><option value="">every day</option>{#each days as d (d)}<option value={d}>{d}</option>{/each}</select>
      </label>
      {#if count}<button class="btn btn--ghost" type="button" onclick={() => (filter = { ...NO_FILTER })}>Clear</button>{/if}
    </div>
  {/if}
</div>
