<!--
  v4 extractions.html: the extractor's model calls. v5 adds server-side
  filters (model, dates, outcome, channel, member, text), deep-linked
  through this page's query string.
-->
<script lang="ts">
  import TokenUsage from '../logs/TokenUsage.svelte';
  import PageLine from '../shell/PageLine.svelte';
  import Name from '../names/Name.svelte';
  import type { Channel, Extractions } from '@kanade/api-types';
  import { activeCount, OUTCOME_LABEL, outcomeTone, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';
  import LogTime from '../logs/LogTime.svelte';
  import { duration } from '../logs/format';
  import type { AdminWeek } from '../store.svelte';
  import RescanPanel from './RescanPanel.svelte';

  let { store, search = '', onsearch }: { store: AdminWeek; search?: string; onsearch?: (search: string) => void } = $props();
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');

  const filter = $derived(parseFilter(search, { chat: false }));
  // A pasted Chat link's tool/latency keys leave the URL rather than linger as dead chips.
  $effect(() => {
    const clean = toSearch(filter);
    if (clean !== toSearch(parseFilter(search))) onsearch?.(clean);
  });
  const extractions = $derived(new Resource<Extractions>(`/api/admin/extractions${toSearch(filter)}`));
  const targets = new Resource<Channel[]>('/api/admin/rescan/targets');
  $effect(() => void extractions.load());
  $effect(() => void targets.load());
  let last = $state<Extractions | null>(null);
  $effect(() => {
    if (extractions.data) last = extractions.data;
  });
  // A refused filter shows its error alone; `last` still feeds the filter facets.
  const view = $derived(extractions.error ? null : last);

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
  const rows = $derived(view?.rows ?? []);
  const shown = $derived(paged(rows, page));
  const filtered = $derived(activeCount(filter) > 0);
</script>

<PageLine title={view ? 'Extractions' : ''}>
  <h1>{#if view}{#if filtered}<span class="pageline__num">{rows.length}</span> of <span class="pageline__num">{view.total}</span>{:else}<span class="pageline__num">{view.total}</span>{/if} model calls{:else}Extractions{/if}</h1>
  <p class="pageline__context">for prompt tuning</p>
  {#snippet side()}
    {#if view}<span class="chip chip--mono">{view.model}</span>{/if}
  {/snippet}
</PageLine>

<PaneWindow title="Calls" bind:query searchLabel="Search calls" placeholder="message, id…">
  <details class="rescan-box">
    <summary class="btn">Re-read the party channels</summary>
    <RescanPanel targets={targets.data ?? []} />
  </details>
  <LogFilters {filter} facets={last?.facets ?? null} members={store.members} week={store.week} onchange={apply} />
  {#if extractions.error}
    <p class="flash flash--error" role="alert">{extractions.error}</p>
  {/if}
  {#if view}
    {#if rows.length === 0}
      <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
    {:else}
      <div class="table-wrap">
        <table>
          <caption class="vh">Extraction calls, newest first</caption>
          <thead>
            <tr><th scope="col">When</th><th scope="col">Channel</th><th scope="col">Outcome</th><th scope="col">Model</th><th scope="col" class="num">Latency</th><th scope="col" class="num">Tokens</th><th scope="col" class="num">Messages</th><th scope="col" class="num">Changes</th><th scope="col"><span class="vh">Open</span></th></tr>
          </thead>
          <tbody>
            {#each shown.rows as row (row.id)}
              <tr>
                <th scope="row" class="mono"><LogTime at={row.at} timeZone={tz} /></th>
                <td class="log__who">{#if row.channel_id}<Name kind="channel" id={row.channel_id} name={row.channel} clip />{:else}—{/if}</td>
                <td><span class="tone tone--{outcomeTone(row.outcome)}">{OUTCOME_LABEL[row.outcome] ?? row.outcome}</span></td>
                <td class="mono log__clip" title={row.model}>{row.model}</td>
                <td class="num log__nowrap">{duration(row.latency_ms)}</td>
                <td class="num log__nowrap"><TokenUsage prompt={row.prompt_tokens} completion={row.completion_tokens} reasoning={row.reasoning_tokens} /></td>
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
  /* Aliases and durations never wrap mid-word; a long alias is cut, its tooltip whole. */
  .log__clip {
    max-width: 14rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .log__nowrap {
    white-space: nowrap;
  }

  .log__who {
    max-width: 14rem;
  }

  .rescan-box {
    margin: 0.2rem 0 0.8rem;
  }

  .rescan-box[open] {
    padding-bottom: 0.6rem;
    border-bottom: 2px solid var(--line-soft);
  }
</style>
