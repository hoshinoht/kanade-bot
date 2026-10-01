<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import type { ReminderRow, Reminders } from '@kanade/api-types';
  import { BossTag } from '@kanade/ui';
  import Pager from '../pages/Pager.svelte';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { paged } from '../pages/paging';
  import { Resource } from '../resource.svelte';

  let { run = '' }: { run?: string } = $props();

  const reminders = new Resource<Reminders>('/api/admin/reminders');
  $effect(() => void reminders.load());

  let query = $state('');
  const q = $derived(query.trim().toLowerCase());
  const keep = (row: ReminderRow) =>
    (!run || row.run_id === run) &&
    (!q ||
      [row.kind, row.run_short_id, ...row.party, ...row.bosses.flatMap((b) => [b.token, b.name])].some((t) => t.toLowerCase().includes(q)));
  // Filters over both lists: the kind of card, the run, a member, the day it fires.
  let kind = $state('');
  let runFilter = $state('');
  let member = $state('');
  let day = $state('');
  const dayOf = (row: ReminderRow) => row.at.replace(/\s+\d{1,2}:\d{2}$/, '');
  const runName = (row: ReminderRow) => `${row.bosses.map((b) => b.token).join(' + ')} #${row.run_short_id}`;
  const all = $derived([...(reminders.data?.upcoming ?? []), ...(reminders.data?.sent ?? [])]);
  const distinct = (values: string[]) => [...new Set(values)];
  const kinds = $derived(distinct(all.map((r) => r.kind)));
  const runs = $derived(distinct(all.map((r) => r.run_id)).map((id) => ({ id, label: runName(all.find((r) => r.run_id === id)!) })));
  const people = $derived(distinct(all.flatMap((r) => r.party)).sort((a, b) => a.localeCompare(b)));
  const days = $derived(distinct(all.map(dayOf)));
  const narrow = (row: ReminderRow) =>
    keep(row) && (!kind || row.kind === kind) && (!runFilter || row.run_id === runFilter) && (!member || row.party.includes(member)) && (!day || dayOf(row) === day);
  const upcoming = $derived((reminders.data?.upcoming ?? []).filter(narrow));
  const sent = $derived((reminders.data?.sent ?? []).filter(narrow));
  const filtered = $derived(Boolean(kind || runFilter || member || day));
  const SIZE = 15;
  let page = $state(1);
  let queuedPage = $state(1);
  $effect(() => {
    void [q, run, kind, runFilter, member, day];
    page = 1;
    queuedPage = 1;
  });
  const sentPage = $derived(paged(sent, page, SIZE));
  const queued = $derived(paged(upcoming, queuedPage, SIZE));
  // One line per row: the first few of the party, the rest behind "+N".
  const SHOWN = 3;
  const runLabel = $derived(run ? (reminders.data?.upcoming.concat(reminders.data.sent).find((r) => r.run_id === run)?.run_short_id ?? run) : '');
  const STATE_WORDS = { queued: 'queued', due: 'due now', sent: 'sent', stale: 'stale — retired without posting' } as const;
</script>

<PageLine title={reminders.data ? 'Reminders' : ''}>
  <h1>{reminders.data ? `${reminders.data.upcoming.length} queued, ${reminders.data.sent.length} sent` : 'Reminders'}</h1>
  <p class="pageline__context">every message the bot will post, or already posted</p>
  {#if run}
    <div class="page-head__side">
      <span class="chip chip--mono">run #{runLabel}</span>
      <a class="btn" href="/reminders">Show every run</a>
    </div>
  {/if}
</PageLine>

{#snippet table(rows: ReminderRow[], fired: boolean, caption: string)}
  <div class="table-wrap">
    <table>
      <caption class="vh">{caption}</caption>
      <thead>
        <tr>
          <th scope="col">{fired ? 'Fired' : 'Fires'}</th>
          <th scope="col">Kind</th>
          <th scope="col">Bosses</th>
          <th scope="col">Run</th>
          <th scope="col">Party</th>
          <th scope="col">{fired ? 'Message' : 'Status'}</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row (row.id)}
          {@const rest = row.party.slice(SHOWN)}
          <tr class="reminder">
            <td class="mono">{row.at}</td>
            <td class="mono">{row.kind}</td>
            <th scope="row"><span class="reminder__bosses">{#each row.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</span></th>
            <td><a class="id" href="/reminders?run={encodeURIComponent(row.run_id)}">#{row.run_short_id}</a></td>
            <td>
              <span class="reminder__party">
                {#each row.party.slice(0, SHOWN) as name, i (i)}<span class="chip">{name}</span>{/each}
                {#if rest.length}<span class="chip chip--mono" title={rest.join(', ')}>+{rest.length}<span class="vh">: {rest.join(', ')}</span></span>{/if}
              </span>
            </td>
            <td>
              {#if row.state === 'sent' && row.url}
                <a href={row.url} target="_blank" rel="noopener noreferrer">open in Discord</a>
              {:else}
                <span class="tone tone--{row.state === 'stale' ? 'danger' : row.state === 'due' ? 'warning' : row.state === 'sent' ? 'success' : 'neutral'}">{STATE_WORDS[row.state]}</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/snippet}

<PaneWindow title="Reminders" bind:query searchLabel="Search reminders" placeholder="boss, member, run id…">
  {#if reminders.error}
    <p class="flash flash--error" role="alert">{reminders.error}</p>
  {:else if reminders.data}
    <div class="filters reminder__filters" role="group" aria-label="Filter reminders">
      <label class="field"><span>Kind</span>
        <select bind:value={kind}><option value="">every kind</option>{#each kinds as k (k)}<option value={k}>{k}</option>{/each}</select>
      </label>
      <label class="field"><span>Run</span>
        <select bind:value={runFilter}><option value="">every run</option>{#each runs as r (r.id)}<option value={r.id}>{r.label}</option>{/each}</select>
      </label>
      <label class="field"><span>Member</span>
        <select bind:value={member}><option value="">anyone</option>{#each people as p (p)}<option value={p}>{p}</option>{/each}</select>
      </label>
      <label class="field"><span>Day</span>
        <select bind:value={day}><option value="">every day</option>{#each days as d (d)}<option value={d}>{d}</option>{/each}</select>
      </label>
      {#if filtered}<button class="btn btn--ghost" type="button" onclick={() => ((kind = ''), (runFilter = ''), (member = ''), (day = ''))}>Clear</button>{/if}
    </div>
    <h3 class="pane__section">Queued <span class="id">{upcoming.length}</span></h3>
    {#if upcoming.length}{@render table(queued.rows, false, 'Queued reminders')}
      <Pager bind:page={queuedPage} pages={queued.pages} total={upcoming.length} size={SIZE} noun="queued card" back="← Earlier" forward="Later →" />
    {:else}<p class="note">Nothing pending{q || filtered ? ' that matches' : ''}.</p>{/if}
    <h3 class="pane__section">Sent <span class="id">{sent.length}</span></h3>
    {#if sent.length}{@render table(sentPage.rows, true, 'Sent reminders')}
      <Pager bind:page pages={sentPage.pages} total={sent.length} size={SIZE} noun="sent card" />{:else}<p class="note">Nothing posted yet{q || filtered ? ' that matches' : ''}.</p>{/if}
  {/if}
</PaneWindow>

<style>
  /* One line per reminder (area principle): bosses inline, the party cut to a few chips. */
  .reminder > :global(td),
  .reminder > :global(th) {
    white-space: nowrap;
    vertical-align: middle;
  }

  .reminder__bosses,
  .reminder__party {
    display: inline-flex;
    flex-wrap: nowrap;
    align-items: center;
    gap: 0.3rem;
  }

  .reminder__filters {
    margin-bottom: 0.2rem;
  }
</style>

