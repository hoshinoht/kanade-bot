<!--
  The log filter bar (Chat, Extractions): one row under the window's title
  bar — "Filters (n)", the active filters as removable chips, Clear — with
  every field in a panel only while open (area budget: rows > filters >
  summary). The text search lives on the title bar, as before.
-->
<script lang="ts">
  import type { LogFacets, Member, Week } from '@kanade/api-types';
  import { activeCount, NO_LOG_FILTER, OUTCOME_LABEL, preset, type LogFilter, type Preset } from './filters';
  import { directory, memberLabel } from '../names/directory.svelte';
  import { Icon } from '@kanade/ui';

  let {
    filter,
    facets,
    members,
    week,
    chat = false,
    onchange,
  }: {
    filter: LogFilter;
    facets: LogFacets | null;
    members: Member[];
    week: Week | null;
    /** Chat adds tool used and minimum latency. */
    chat?: boolean;
    onchange: (next: LogFilter) => void;
  } = $props();
  const uid = $props.id();
  let open = $state(false);
  let root = $state<HTMLDivElement>();
  let toggle = $state<HTMLButtonElement>();

  function close(refocus: boolean) {
    open = false;
    if (refocus) toggle?.focus({ preventScroll: true });
  }
  // A press outside the bar (button, chips, Clear, panel) closes the panel.
  $effect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => {
      if (!root?.contains(event.target as Node)) close(false);
    };
    document.addEventListener('pointerdown', away, true);
    return () => document.removeEventListener('pointerdown', away, true);
  });

  const set = (patch: Partial<LogFilter>) => onchange({ ...filter, ...patch });
  const count = $derived(activeCount({ ...filter, q: '' }));
  // Filter chips name what they filter by, never the raw id.
  const channelName = (id: string) => directory.label('channel', id, facets?.channels.find((c) => c.id === id)?.name ?? '');
  const memberName = (id: string) => memberLabel(members, id);

  type Chip = { key: string; label: string; clear: Partial<LogFilter> };
  const chips = $derived.by(() => {
    const out: Chip[] = [];
    if (filter.model) out.push({ key: 'model', label: `Model: ${filter.model}`, clear: { model: '' } });
    if (filter.from || filter.to)
      out.push({
        key: 'dates',
        label: filter.from === filter.to ? `On ${filter.from}` : `${filter.from || '…'} → ${filter.to || '…'}`,
        clear: { from: '', to: '' },
      });
    if (filter.outcome.length)
      out.push({ key: 'outcome', label: `Outcome: ${filter.outcome.map((o) => OUTCOME_LABEL[o] ?? o).join(', ')}`, clear: { outcome: [] } });
    if (filter.channel) out.push({ key: 'channel', label: `Channel: ${channelName(filter.channel)}`, clear: { channel: '' } });
    if (filter.member) out.push({ key: 'member', label: `Member: ${memberName(filter.member)}`, clear: { member: '' } });
    if (filter.tool) out.push({ key: 'tool', label: `Tool: ${filter.tool}`, clear: { tool: '' } });
    if (filter.min_ms) out.push({ key: 'min_ms', label: `≥ ${filter.min_ms} ms`, clear: { min_ms: '' } });
    return out;
  });

  function usePreset(which: Preset) {
    const range = week ? preset(which, week) : null;
    if (range) set(range);
  }

  function toggleOutcome(outcome: string, on: boolean) {
    set({ outcome: on ? [...filter.outcome, outcome] : filter.outcome.filter((o) => o !== outcome) });
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="logfilters"
  bind:this={root}
  onkeydown={(event) => {
    if (open && event.key === 'Escape') {
      event.stopPropagation();
      close(true);
    }
  }}
>
  <div class="logfilters__row">
    <button type="button" class="btn" aria-expanded={open} aria-controls="{uid}-panel" bind:this={toggle} onclick={() => (open = !open)}
      ><Icon name="filter" /><span>Filters ({count})</span></button
    >
    {#each chips as chip (chip.key)}
      <button type="button" class="chip logfilters__chip" onclick={() => set(chip.clear)}
        >{chip.label}<span aria-hidden="true"> ×</span><span class="vh"> — remove</span></button
      >
    {/each}
    {#if count || filter.q}
      <button type="button" class="btn btn--ghost" onclick={() => onchange({ ...NO_LOG_FILTER, outcome: [] })}>Clear</button>
    {/if}
  </div>
  {#if open}
    <div class="filters logfilters__panel" id="{uid}-panel" role="group" aria-label="Filters">
      <label class="field"
        ><span>Model</span>
        <select value={filter.model} onchange={(e) => set({ model: e.currentTarget.value })}>
          <option value="">any model</option>
          {#each facets?.models ?? [] as m (m)}<option value={m}>{m}</option>{/each}
        </select>
      </label>
      <label class="field"><span>From</span><input type="date" value={filter.from} onchange={(e) => set({ from: e.currentTarget.value })} /></label>
      <label class="field"><span>To</span><input type="date" value={filter.to} onchange={(e) => set({ to: e.currentTarget.value })} /></label>
      <div class="field">
        <span id="{uid}-presets">Dates ({week?.timezone ?? 'guild time'})</span>
        <div class="logfilters__presets" role="group" aria-labelledby="{uid}-presets">
          <button type="button" class="btn" onclick={() => usePreset('today')}>Today</button>
          <button type="button" class="btn" onclick={() => usePreset('week')}>This boss week</button>
          <button type="button" class="btn" onclick={() => usePreset('7d')}>7 days</button>
        </div>
      </div>
      <label class="field"
        ><span>Channel</span>
        <select value={filter.channel} onchange={(e) => set({ channel: e.currentTarget.value })}>
          <option value="">every channel</option>
          {#each facets?.channels ?? [] as c (c.id)}<option value={c.id}>{directory.label('channel', c.id, c.name)}</option>{/each}
        </select>
      </label>
      <label class="field"
        ><span>Member</span>
        <select value={filter.member} onchange={(e) => set({ member: e.currentTarget.value })}>
          <option value="">anyone</option>
          {#each members as m (m.id)}<option value={m.id}>{memberLabel(members, m.id)}</option>{/each}
        </select>
      </label>
      {#if chat}
        <label class="field"
          ><span>Tool used</span>
          <select value={filter.tool} onchange={(e) => set({ tool: e.currentTarget.value })}>
            <option value="">any tool</option>
            {#each facets?.tools ?? [] as t (t)}<option value={t}>{t}</option>{/each}
          </select>
        </label>
        <label class="field"
          ><span>At least (ms)</span><input type="number" min="0" step="100" inputmode="numeric" value={filter.min_ms}
            onchange={(e) => set({ min_ms: e.currentTarget.value })} /></label
        >
      {/if}
      <fieldset class="field logfilters__outcomes">
        <legend>Outcome (any of)</legend>
        {#each facets?.outcomes ?? [] as o (o)}
          <label class="logfilters__outcome"
            ><input type="checkbox" checked={filter.outcome.includes(o)} onchange={(e) => toggleOutcome(o, e.currentTarget.checked)} />
            {OUTCOME_LABEL[o] ?? o}</label
          >
        {/each}
      </fieldset>
    </div>
  {/if}
</div>
