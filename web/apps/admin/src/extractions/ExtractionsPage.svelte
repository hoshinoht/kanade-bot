<!--
  v4 extractions.html: the extractor's model calls. v5 adds server-side
  filters (model, dates, outcome, channel, member, text), deep-linked
  through this page's query string.
-->
<script lang="ts">
  import type { Channel, Extractions } from '@kanade/api-types';
  import { activeCount, OUTCOME_LABEL, outcomeTone, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import RescanPanel from './RescanPanel.svelte';

  let { store, search = '', onsearch }: { store: AdminWeek; search?: string; onsearch?: (search: string) => void } = $props();

  const filter = $derived(parseFilter(search));
  const extractions = $derived(new Resource<Extractions>(`/api/admin/extractions${toSearch({ ...filter, tool: '', min_ms: '' })}`));
  const targets = new Resource<Channel[]>('/api/admin/rescan/targets');
  $effect(() => void extractions.load());
  $effect(() => void targets.load());
  let last = $state<Extractions | null>(null);
  $effect(() => {
    if (extractions.data) last = extractions.data;
  });

  function apply(next: LogFilter) {
    onsearch?.(toSearch(next));
  }

  // svelte-ignore state_referenced_locally
  let query = $state(filter.q);
  $effect(() => {
    const text = query;
    if (text.trim() === filter.q.trim()) return;
    const timer = setTimeout(() => apply({ ...filter, q: text }), 250);
    return () => clearTimeout(timer);
  });
  // A q changed from outside the box (Clear, Back, a deep link) replaces what it shows.
  // svelte-ignore state_referenced_locally
  let seenQ = filter.q;
  $effect(() => {
    const q = filter.q;
    if (q === seenQ) return;
    seenQ = q;
    if (q.trim() !== query.trim()) query = q;
  });

  let page = $state(1);
  $effect(() => {
    void search;
    page = 1;
  });
  const rows = $derived(last?.rows ?? []);
  const shown = $derived(paged(rows, page));
  const filtered = $derived(activeCount(filter) > 0);
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">Prompt tuning</p>
    <h1>{last ? (filtered ? `${rows.length} of ${last.total} model calls` : `${last.total} model calls`) : 'Extractions'}</h1>
  </div>
  {#if last}<span class="chip chip--mono">{last.model}</span>{/if}
</div>

<PaneWindow title="Calls" bind:query searchLabel="Search calls" placeholder="message, id…">
  <details class="rescan-box">
    <summary class="btn">Re-read the party channels</summary>
    <RescanPanel targets={targets.data ?? []} />
  </details>
  <LogFilters {filter} facets={last?.facets ?? null} members={store.members} week={store.week} onchange={apply} />
  {#if extractions.error}
    <p class="flash flash--error" role="alert">{extractions.error}</p>
  {/if}
  {#if last}
    {#if rows.length === 0}
      <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
    {:else}
      <div class="table-wrap">
        <table>
          <caption class="vh">Extraction calls, newest first</caption>
          <thead>
            <tr><th scope="col">When</th><th scope="col">Channel</th><th scope="col">Outcome</th><th scope="col">Model</th><th scope="col" class="num">Latency</th><th scope="col" class="num">Messages</th><th scope="col" class="num">Changes</th><th scope="col"><span class="vh">Open</span></th></tr>
          </thead>
          <tbody>
            {#each shown.rows as row (row.id)}
              <tr>
                <th scope="row" class="mono">{row.at}</th>
                <td>{row.channel ?? '—'}</td>
                <td><span class="status status--{outcomeTone(row.outcome)}">{OUTCOME_LABEL[row.outcome] ?? row.outcome}</span></td>
                <td class="mono">{row.model}</td>
                <td class="num">{row.latency_ms == null ? '—' : `${row.latency_ms.toLocaleString('en')} ms`}</td>
                <td class="num">{row.messages}</td>
                <td class="num">{row.changes}</td>
                <td><a class="btn" href="/extractions/{row.id}" aria-label="Open call {row.short_id}">Open</a></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <Pager bind:page pages={shown.pages} total={rows.length} noun="call" />
    {/if}
  {:else if !extractions.error}
    <p class="note" aria-busy="true">Loading calls…</p>
  {/if}
</PaneWindow>

<style>
  .rescan-box {
    margin: 0.2rem 0 0.8rem;
  }

  .rescan-box[open] {
    padding-bottom: 0.6rem;
    border-bottom: 2px solid var(--line-soft);
  }
</style>
