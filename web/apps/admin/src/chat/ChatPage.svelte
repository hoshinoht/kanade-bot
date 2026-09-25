<!--
  v4 chat.html: the chatbot's interactions. v5 adds server-side filters
  (model, dates, outcome, channel, member, text, tool, minimum latency),
  deep-linked through this page's query string.
-->
<script lang="ts">
  import LogTime from '../logs/LogTime.svelte';
  import { duration } from '../logs/format';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import type { AdminWeek } from '../store.svelte';
  import type { Chat } from '@kanade/api-types';
  import { activeCount, OUTCOME_LABEL, outcomeTone, parseFilter, toSearch, type LogFilter } from '../logs/filters';
  import LogFilters from '../logs/LogFilters.svelte';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';

  let { store, search = '', onsearch }: { store: AdminWeek; search?: string; onsearch?: (search: string) => void } = $props();
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');

  const filter = $derived(parseFilter(search));
  const chat = $derived(new Resource<Chat>(`/api/admin/chat${toSearch(filter)}`));
  $effect(() => void chat.load());
  // Last good read stays on screen while a new filter loads.
  let last = $state<Chat | null>(null);
  $effect(() => {
    if (chat.data) last = chat.data;
  });
  // A refused filter shows its error alone; `last` still feeds the filter facets.
  const view = $derived(chat.error ? null : last);

  function apply(next: LogFilter) {
    onsearch?.(toSearch(next));
  }

  // The title bar's text search, debounced into the URL.
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

<div class="page-head">
  <div>
    <p class="eyebrow">The speech pilot</p>
    <h1>{view ? (filtered ? `${rows.length} of ${view.total} interactions` : `${view.total} interactions`) : 'Chat'}</h1>
  </div>
  {#if view && view.summary.length}
    <div class="statline" aria-label="Per model, for these rows">
      {#each view.summary as m (m.model)}
        <div class="statline__row">
          <span class="statline__model mono">{m.model}</span>
          <span class="statline__pair"><span class="statline__k">answered</span> {m.answered}</span>
          <span class="statline__pair"><span class="statline__k">refused</span> {m.refused}</span>
          <span class="statline__pair"><span class="statline__k">errors</span> {m.errors}</span>
          <span class="statline__pair"><span class="statline__k">p50</span> {m.p50_ms.toLocaleString('en')} ms</span>
          <span class="statline__pair"><span class="statline__k">tool calls</span> {m.tool_calls}</span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<PaneWindow title="Interactions" bind:query searchLabel="Search interactions" placeholder="question, answer…">
  <LogFilters {filter} facets={last?.facets ?? null} members={store.members} week={store.week} chat onchange={apply} />
  {#if chat.error}
    <p class="flash flash--error" role="alert">{chat.error}</p>
  {/if}
  {#if view}
    {#if rows.length === 0}
      <div class="empty"><strong>Nothing matches these filters.</strong>Remove a chip above, or Clear them all.</div>
    {:else}
      <div class="table-wrap">
        <table>
          <caption class="vh">Chatbot interactions, newest first</caption>
          <thead><tr><th scope="col">Question</th><th scope="col">Who</th><th scope="col">When</th><th scope="col">Outcome</th><th scope="col">Model</th><th scope="col" class="num">Took</th></tr></thead>
          <tbody>
            {#each shown.rows as row (row.id)}
              {@const models = row.models.length ? row.models.filter((m, i) => row.models.indexOf(m) === i).join(', ') : '—'}
              <tr>
                <th scope="row"><a href="/chat/{row.id}"><Mentions text={row.asked} plain asked dropBot /></a></th>
                <td class="log__who"><Name kind="member" id={row.member_id || row.member.id} name={row.member.name} clip /><div class="id"><Name kind="channel" id={row.channel_id} name={row.channel} clip /></div></td>
                <td class="mono"><LogTime at={row.at} timeZone={tz} /></td>
                <td><span class="tone tone--{outcomeTone(row.outcome)}">{OUTCOME_LABEL[row.outcome] ?? row.outcome}</span></td>
                <td class="mono log__clip" title={models}>{models}</td>
                <td class="num log__nowrap">{duration(row.latency_ms)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <Pager bind:page pages={shown.pages} total={rows.length} noun="interaction" />
    {/if}
  {:else if !chat.error}
    <p class="note" aria-busy="true">Loading interactions…</p>
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

  .statline {
    flex: 1 0 100%;
    display: grid;
    gap: 0.3rem;
    padding-top: 0.45rem;
    border-top: 2px solid var(--line-soft);
  }

  .statline__row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.2rem 1rem;
  }

  .statline__model {
    font-size: var(--fs-small);
    font-weight: 600;
  }

  .statline__pair {
    font-size: var(--fs-mini);
    font-family: var(--mono);
  }

  .statline__k {
    font-size: var(--fs-micro);
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--faint);
  }
</style>
