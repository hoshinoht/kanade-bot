<script lang="ts">
  import type { ChangeRecord } from '@kanade/api-types';
  import { Modal, SidePane } from '@kanade/ui';
  import { SURFACE_LABELS, describe, localAt, type Names } from './describe';

  let {
    wide,
    record,
    week,
    timezone,
    names,
    onclose,
    onrevert,
    onrestore,
    onraw,
  }: {
    wide: boolean;
    record: ChangeRecord;
    week: string;
    timezone: string;
    names: Names;
    onclose: () => void;
    onrevert: (record: ChangeRecord) => void;
    onrestore: (week: string, record: ChangeRecord) => void;
    onraw: (record: ChangeRecord) => void;
  } = $props();

  const label = (record: ChangeRecord) => `Change #${record.seq}`;

  type Row = ChangeRecord['rows'][number];
  const shown = (value: unknown) => (value === undefined ? '—' : typeof value === 'string' ? value : JSON.stringify(value));
  // Only the fields that differ; the full before/after stays in the raw view.
  function changedFields(row: Row) {
    const before = (row.before ?? {}) as Record<string, unknown>;
    const after = (row.after ?? {}) as Record<string, unknown>;
    const names = [...new Set([...Object.keys(before), ...Object.keys(after)])];
    return names
      .filter((name) => JSON.stringify(before[name]) !== JSON.stringify(after[name]))
      .map((name) => ({ name, was: row.before ? shown(before[name]) : '—', now: row.after ? shown(after[name]) : '—' }));
  }
</script>

<svelte:window
  onkeydown={(event) => {
    // A confirmation dialog owns Escape while it is open; otherwise the wide
    // pane closes like the other M3E side panes.
    if (wide && event.key === 'Escape' && !document.querySelector('dialog[open]')) {
      event.preventDefault();
      onclose();
    }
  }}
/>

{#snippet content()}
  <div class="history-detail__content">
    <p class="cap">Change #{record.seq} · {SURFACE_LABELS[record.surface] ?? record.surface} · {localAt(record.at, timezone)}</p>
    <h2>{describe(record, names, timezone)[0] ?? label(record)}</h2>
    <p class="history-detail__meta">revision <span class="mono">{record.revision}</span> · {record.rows.length} row{record.rows.length === 1 ? '' : 's'} · hash <span class="mono">{record.hash.slice(0, 12)}</span></p>

    <section class="history-detail__rows" aria-labelledby="history-rows-{record.seq}">
      <h3 id="history-rows-{record.seq}">Rows</h3>
      {#each record.rows as row, i (i)}
        <article class="history-diff">
          <p class="mono">{'id' in row.key ? `${row.key.table}/${row.key.id}` : `rsvps/${row.key.run_id}/${row.key.user_id}`}{#if !row.before} · created{:else if !row.after} · removed{/if}</p>
          <dl class="history-diff__fields">
            {#each changedFields(row) as field (field.name)}
              <div class="history-diff__field">
                <dt class="mono" title={field.name}>{field.name}</dt>
                <dd class="history-diff__was" title={field.was}><span class="vh">was </span><s>{field.was}</s></dd>
                <dd class="history-diff__arrow" aria-hidden="true">→</dd>
                <dd class="history-diff__now" title={field.now}><span class="vh">now </span>{field.now}</dd>
              </div>
            {:else}
              <div class="history-diff__field"><dt class="mono">—</dt><dd class="history-diff__now">no field changed</dd></div>
            {/each}
          </dl>
        </article>
      {/each}
    </section>

    <button class="linklike history-detail__raw" type="button" onclick={() => onraw(record)}>Show raw JSON</button>

    <div class="history-detail__actions">
      <button class="btn btn--danger" type="button" data-history-revert={record.seq} onclick={() => onrevert(record)}>Revert…</button>
      {#if week}<button class="btn btn--ghost" type="button" onclick={() => onrestore(week, record)}>Restore week to here…</button>{/if}
    </div>
  </div>
{/snippet}

{#if wide}
  <SidePane label="Change details" className="side-pane--history">
    <header class="history-detail__head">
      <span class="cap">History</span>
      <button class="btn btn--ghost history-detail__close" type="button" aria-label="Close change details" onclick={onclose}>×</button>
    </header>
    {@render content()}
  </SidePane>
{:else}
  <Modal open title={label(record)} eyebrow="History" narrow className="history-detail" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
