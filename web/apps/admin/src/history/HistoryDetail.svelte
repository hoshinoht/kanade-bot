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
  }: {
    wide: boolean;
    record: ChangeRecord;
    week: string;
    timezone: string;
    names: Names;
    onclose: () => void;
    onrevert: (record: ChangeRecord) => void;
    onrestore: (week: string, record: ChangeRecord) => void;
  } = $props();

  const label = (record: ChangeRecord) => `Change #${record.seq}`;
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
          <p class="mono">{'id' in row.key ? `${row.key.table}/${row.key.id}` : `rsvps/${row.key.run_id}/${row.key.user_id}`}</p>
          <div class="history-diff__values">
            <pre><s>was {JSON.stringify(row.before, null, 1)}</s></pre>
            <span aria-hidden="true">→</span>
            <pre>now {JSON.stringify(row.after, null, 1)}</pre>
          </div>
        </article>
      {/each}
    </section>

    <details class="history-detail__raw">
      <summary>Show raw JSON</summary>
      <pre>{JSON.stringify(record, null, 2)}</pre>
    </details>

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
