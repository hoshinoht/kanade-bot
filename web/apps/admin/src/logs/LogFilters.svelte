<!--
  The log filter bar (Chat, Extractions, Rewrites): one row under the window's title
  bar — "Filters (n)", the active filters as removable chips, Clear — with
  every field in a panel only while open (area budget: rows > filters >
  summary). The text search lives on the title bar, as before.
-->
<script lang="ts">
  import type { LogFacets, Member, Week } from '@kanade/api-types';
  import '@kanade/ui/styles/date-picker.scss';
  import { activeCount, KIND_LABEL, NO_LOG_FILTER, OUTCOME_LABEL, STAGE_LABEL, type LogFilter } from './filters';
  import { directory, memberLabel } from '../names/directory.svelte';
  import '@kanade/ui/styles/select.scss';
  import { DatePicker, Icon, Select, serverClock, type SelectOption } from '@kanade/ui';

  let {
    filter,
    facets,
    members,
    week,
    chat = false,
    rewrites = null,
    onchange,
  }: {
    filter: LogFilter;
    facets: LogFacets | null;
    members: Member[];
    week: Week | null;
    /** Chat adds tool used and minimum latency. */
    chat?: boolean;
    /** Rewrites: kind and stage in place of channel and member; outcomes are verdicts. */
    rewrites?: { kinds: string[]; stages: string[] } | null;
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
    if (filter.kind) out.push({ key: 'kind', label: `Kind: ${KIND_LABEL[filter.kind] ?? filter.kind}`, clear: { kind: '' } });
    if (filter.stage) out.push({ key: 'stage', label: `Stage: ${STAGE_LABEL[filter.stage] ?? filter.stage}`, clear: { stage: '' } });
    if (filter.outcome.length)
      out.push({
        key: 'outcome',
        label: `${rewrites ? 'Verdict' : 'Outcome'}: ${filter.outcome.map((o) => OUTCOME_LABEL[o] ?? o).join(', ')}`,
        clear: { outcome: [] },
      });
    if (filter.channel) out.push({ key: 'channel', label: `Channel: ${channelName(filter.channel)}`, clear: { channel: '' } });
    if (filter.member) out.push({ key: 'member', label: `Member: ${memberName(filter.member)}`, clear: { member: '' } });
    if (filter.tool) out.push({ key: 'tool', label: `Tool: ${filter.tool}`, clear: { tool: '' } });
    if (filter.min_ms) out.push({ key: 'min_ms', label: `≥ ${filter.min_ms} ms`, clear: { min_ms: '' } });
    return out;
  });

  const modelOptions = $derived<SelectOption[]>([{ value: '', label: 'any model' }, ...(facets?.models ?? []).map((m) => ({ value: m, label: m }))]);
  const channelOptions = $derived<SelectOption[]>([
    { value: '', label: 'every channel' },
    ...(facets?.channels ?? []).map((c) => ({ value: c.id, label: directory.label('channel', c.id, c.name) })),
  ]);
  const memberOptions = $derived<SelectOption[]>([
    { value: '', label: 'anyone', icon: 'users' },
    ...members.map((m) => {
      const name = memberLabel(members, m.id);
      return { value: m.id, label: name, mono: name.slice(0, 1).toUpperCase() };
    }),
  ]);
  const toolOptions = $derived<SelectOption[]>([{ value: '', label: 'any tool' }, ...(facets?.tools ?? []).map((t) => ({ value: t, label: t }))]);
  const kindOptions = $derived<SelectOption[]>([
    { value: '', label: 'any kind' },
    ...(rewrites?.kinds ?? []).map((k) => ({ value: k, label: KIND_LABEL[k] ?? k })),
  ]);
  const stageOptions = $derived<SelectOption[]>([
    { value: '', label: 'any stage' },
    ...(rewrites?.stages ?? []).map((k) => ({ value: k, label: STAGE_LABEL[k] ?? k })),
  ]);

  // Today and the boss week come from the server's clock, never the browser's.
  const clock = $derived(serverClock(week));

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
    <!-- The range sits in the row beside the filters (P_Dates); its trigger shows and clears it. -->
    <div class="logfilters__dates">
      <DatePicker label="Dates" {clock} from={filter.from} to={filter.to} onrange={(from, to) => set({ from, to })} />
    </div>
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
      <Select size="bar" label="Model" options={modelOptions} value={filter.model} onchange={(model) => set({ model })} noun="models" />
      {#if rewrites}
        <Select size="bar" label="Kind" options={kindOptions} value={filter.kind} onchange={(kind) => set({ kind })} noun="kinds" />
        <Select size="bar" label="Stage" options={stageOptions} value={filter.stage} onchange={(stage) => set({ stage })} noun="stages" />
      {:else}
        <Select size="bar" label="Channel" options={channelOptions} value={filter.channel} onchange={(channel) => set({ channel })} noun="channels" />
        <Select size="bar" label="Member" options={memberOptions} value={filter.member} onchange={(member) => set({ member })} noun="members" />
      {/if}
      {#if chat}
        <Select size="bar" label="Tool used" options={toolOptions} value={filter.tool} onchange={(tool) => set({ tool })} noun="tools" />
        <label class="field"
          ><span>At least (ms)</span><input type="number" min="0" step="100" inputmode="numeric" value={filter.min_ms}
            onchange={(e) => set({ min_ms: e.currentTarget.value })} /></label
        >
      {/if}
      <fieldset class="field logfilters__outcomes">
        <legend>{rewrites ? 'Verdict' : 'Outcome'} (any of)</legend>
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
