<!--
  A run's change log, as an IDE's commit list: newest first, one row per
  change (its summary over avatar · actor · relative time · via surface ·
  #seq); a row opens to the run's fields before → after. "By field" narrows
  the log to the changes that touched one field (the old blame question:
  the first row left is the last change). Loads when it first comes into view.
-->
<script lang="ts">
  import type { ChangeRecord, HistoryPage, Member } from '@kanade/api-types';
  import { initial } from '@kanade/ui';
  import { SvelteSet } from 'svelte/reactivity';
  import { SURFACE_LABELS, actorName, describe, localAt, relativeAt } from '../history/describe';
  import { memberLabel } from '../names/directory.svelte';
  import { send } from '../resource.svelte';
  import { fieldChanges, fieldLabel, fieldValue, inRun, runRows } from './runLog';

  let {
    runId,
    title,
    members,
    timezone,
    now,
    channel,
  }: {
    runId: string;
    /** The run's title, for answers in records that carry no run row. */
    title: string;
    members: Member[];
    timezone: string;
    /** The server's clock (the week's `generated_at`) for "3 h ago". */
    now: string;
    channel?: { id: string; name: string };
  } = $props();

  const uid = $props.id();
  let records = $state<ChangeRecord[]>([]);
  let nextBefore = $state<number | null>(null);
  let loaded = $state(false);
  let loading = $state(false);
  let error = $state('');
  let field = $state('');
  const opened = new SvelteSet<number>();

  const names = (id: string) => memberLabel(members, id);
  const known = (id: string) => members.some((m) => m.id === id);
  const ctx = $derived({ names, timeZone: timezone, channel });

  async function load(more = false) {
    if (loading) return;
    loading = true;
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a request's query, built and sent, never state
    const params = new URLSearchParams({ run: runId, limit: '20' });
    if (more && nextBefore !== null) params.set('before', String(nextBefore));
    const result = await send((c) => c.get<HistoryPage>(`/api/admin/history?${params}`));
    loading = false;
    if (!result.ok) {
      error = result.message;
      return;
    }
    error = '';
    records = more ? [...records, ...result.value.records] : result.value.records;
    nextBefore = result.value.next_before;
    loaded = true;
  }

  // In the pane it is in view at once; at the foot of the phone sheet it waits to be scrolled to.
  function whenSeen(node: HTMLElement) {
    const seen = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        seen.disconnect();
        void load();
      }
    });
    seen.observe(node);
    return () => seen.disconnect();
  }

  const entries = $derived(
    records.map((record) => {
      const changes = fieldChanges(record, runId);
      const lines = describe({ ...record, rows: runRows(record, runId) }, names, timezone, () => title);
      return { record, changes, summary: lines.map((line) => inRun(line, title)).join(' · ') || `${fieldLabel(changes[0]?.field ?? '', names) || 'Run'} changed` };
    }),
  );
  // The fields the loaded changes touched, in first-seen order.
  const fields = $derived([...new Set(entries.flatMap((e) => e.changes.map((c) => c.field)))]);
  const shown = $derived(field ? entries.filter((e) => e.changes.some((c) => c.field === field)) : entries);
  const who = (record: ChangeRecord) => actorName(record.actor, names, known);
</script>

<section class="runlog" aria-labelledby="{uid}-title" {@attach whenSeen}>
  <div class="runlog__bar">
    <h3 class="cap runlog__title" id="{uid}-title">Changes</h3>
    {#if fields.length > 1}
      <label class="btn runlog__filter" class:runlog__filter--on={field}
        ><span class="runlog__filter-label">By field</span>
        <select bind:value={field}>
          <option value="">every field</option>
          {#each fields as f (f)}<option value={f}>{fieldLabel(f, names)}</option>{/each}
        </select></label
      >
    {/if}
  </div>
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  {#if !loaded}
    {#if !error}<p class="note" aria-busy="true">Loading…</p>{/if}
  {:else if records.length === 0}
    <p class="note">Nothing has changed since it was materialised.</p>
  {:else}
    <ol class="runlog__list">
      {#each shown as entry (entry.record.seq)}
        {@const record = entry.record}
        {@const open = opened.has(record.seq)}
        <li class="runlog__item" data-runlog={record.seq}>
          <button
            type="button"
            class="runlog__row"
            aria-expanded={open}
            aria-controls="{uid}-c{record.seq}"
            onclick={() => (open ? opened.delete(record.seq) : opened.add(record.seq))}
          >
            <span class="runlog__summary">{entry.summary}</span>
            <span class="runlog__meta">
              <span class="runlog__avatar" aria-hidden="true">{initial(who(record))}</span>
              <span class="runlog__actor">{who(record)}</span>
              <span aria-hidden="true">·</span>
              <time datetime={record.at} title={localAt(record.at, timezone)}>{relativeAt(record.at, now) || localAt(record.at, timezone)}</time>
              <span aria-hidden="true">·</span>
              <span>via {SURFACE_LABELS[record.surface] ?? record.surface}</span>
              {#if record.refs.length}<span aria-hidden="true">·</span><span>reverts {record.refs.map((r) => `#${r.seq}`).join(', ')}</span>{/if}
              <span class="runlog__seq mono">#{record.seq}</span>
            </span>
          </button>
          <dl class="runlog__diff" id="{uid}-c{record.seq}" hidden={!open}>
            {#each entry.changes as change (change.field)}
              <div class="runlog__field" class:runlog__field--picked={change.field === field}>
                <dt>{fieldLabel(change.field, names)}</dt>
                <dd>
                  {#if change.field !== 'created'}<s class="runlog__was"><span class="vh">was </span>{fieldValue(change.field, change.before, ctx)}</s>
                    <span class="runlog__arrow" aria-hidden="true">→</span>{/if}
                  <span class="runlog__now"><span class="vh">now </span>{fieldValue(change.field, change.after, ctx)}</span>
                </dd>
              </div>
            {:else}
              <div class="runlog__field"><dt>Fields</dt><dd>no field of this run changed</dd></div>
            {/each}
            <div class="runlog__field runlog__field--when"><dt>When</dt><dd class="mono">{localAt(record.at, timezone)}</dd></div>
          </dl>
        </li>
      {/each}
    </ol>
    {#if field && shown.length === 0}<p class="note">No loaded change touched this field.</p>{/if}
    {#if nextBefore !== null}<button class="btn runlog__more" type="button" aria-disabled={loading} onclick={() => void load(true)}>Older changes</button>{/if}
  {/if}
</section>

<style>
  /* Rows always stack (summary over meta) so the log fits the pane at any width. */
  .runlog {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    min-width: 0;
    container-type: inline-size;
  }

  .runlog__bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem 0.75rem;
  }

  .runlog__title {
    flex: 1 1 auto;
    margin: 0;
  }

  .runlog__filter {
    gap: 0.25rem;
    min-height: 2rem;
    max-width: 100%;
    padding: 0 0.35rem 0 0.75rem;
    border-radius: 1rem;
    cursor: pointer;
  }

  .runlog__filter--on {
    background: var(--select);
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
  }

  .runlog__filter-label {
    font-weight: 600;
    white-space: nowrap;
  }

  .runlog__filter-label::after {
    content: ':';
  }

  .runlog__filter select {
    field-sizing: content;
    min-width: 0;
    max-width: 12rem;
    min-height: 0;
    padding: 0.2rem 0.1rem;
    border: 0;
    background: transparent;
    box-shadow: none;
    color: inherit;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }

  .runlog__filter:focus-within {
    outline: 2px solid var(--win-ink);
    outline-offset: 1px;
  }

  .runlog__list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .runlog__row {
    display: flex;
    flex-direction: column;
    gap: 3px;
    width: 100%;
    min-height: 44px;
    padding: 8px 10px;
    border: 0;
    border-radius: 6px;
    background: var(--row);
    color: var(--ink);
    font: inherit;
    line-height: 1.35;
    text-align: start;
    cursor: pointer;
  }

  .runlog__row:hover {
    background: var(--row-hover);
    box-shadow: inset 0 0 0 1.5px var(--line);
  }

  .runlog__row[aria-expanded='true'] {
    border-radius: 6px 6px 0 0;
    background: var(--select);
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
  }

  .runlog__summary {
    min-width: 0;
    overflow: hidden;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .runlog__row[aria-expanded='true'] .runlog__summary {
    white-space: normal;
    overflow-wrap: anywhere;
  }

  .runlog__meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
    min-width: 0;
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  .runlog__avatar {
    display: inline-grid;
    flex: none;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--chip-fill);
    color: var(--ink);
    font-size: var(--fs-mini);
    font-weight: 700;
  }

  .runlog__actor {
    color: var(--ink);
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .runlog__seq {
    margin-inline-start: auto;
  }

  .runlog__diff {
    display: grid;
    gap: 4px;
    margin: 0;
    padding: 8px 10px 10px;
    border-radius: 0 0 6px 6px;
    background: var(--row);
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
    font-size: var(--fs-small);
  }

  .runlog__diff[hidden] {
    display: none;
  }

  .runlog__field {
    display: grid;
    grid-template-columns: minmax(5.5rem, max-content) minmax(0, 1fr);
    gap: 0 0.6rem;
    align-items: baseline;
  }

  .runlog__field dt {
    color: var(--dim-text);
  }

  .runlog__field dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .runlog__field--picked dt {
    color: var(--ink);
    font-weight: 700;
  }

  .runlog__was {
    color: var(--dim-text);
  }

  .runlog__arrow {
    color: var(--dim-text);
  }

  .runlog__now {
    font-weight: 700;
  }

  .runlog__more {
    align-self: flex-start;
  }

  .runlog__more[aria-disabled='true'] {
    cursor: progress;
    opacity: 0.75;
  }

  /* The pane's narrowest width and the phone sheet: the field name sits over its values. */
  @container (max-width: 22rem) {
    .runlog__field {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
