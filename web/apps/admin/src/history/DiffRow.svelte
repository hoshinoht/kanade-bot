<!--
  One History diff row (B_History `.diff`): a head line, then only the
  fields that differ as name, was → now. History shows a record's rows with
  it; Drafts' preview shows the rows a merge would write.
-->
<script lang="ts">
  import type { RowChange } from '@kanade/api-types';
  import type { Snippet } from 'svelte';

  let { row, fid, head }: { row: RowChange; fid: string; head?: Snippet } = $props();

  const shown = (value: unknown) => (value === undefined ? '—' : typeof value === 'string' ? value : JSON.stringify(value));
  // Only the fields that differ; the full before/after stays in the raw view.
  const fields = $derived.by(() => {
    const before = (row.before ?? {}) as Record<string, unknown>;
    const after = (row.after ?? {}) as Record<string, unknown>;
    const names = [...new Set([...Object.keys(before), ...Object.keys(after)])];
    return names
      .filter((name) => JSON.stringify(before[name]) !== JSON.stringify(after[name]))
      .map((name) => ({ name, was: row.before ? shown(before[name]) : '—', now: row.after ? shown(after[name]) : '—' }));
  });
</script>

<article class="history-diff" data-fid={fid}>
  {#if head}{@render head()}{:else}<p class="mono">{'id' in row.key ? `${row.key.table}/${row.key.id}` : `rsvps/${row.key.run_id}/${row.key.user_id}`}{!row.before ? ' · created' : !row.after ? ' · removed' : ''}</p>{/if}
  <dl class="history-diff__fields">
    {#each fields as field (field.name)}
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
