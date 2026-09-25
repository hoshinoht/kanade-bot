<!--
  History (v4 Audit, v5 git-style; docs/v5/history.md): a per-week timeline of
  immutable change records with actor, surface, refs and row diffs; revert a
  change, restore a week to a point, or revert one member's changes, each
  previewed with a conflict report; and the backup checkpoints that anchor
  the hash chain.
-->
<script lang="ts">
  import '@kanade/ui/styles/evidence.scss';
  import type { ChangeRecord, Checkpoints, HistoryPage, RevertPlan } from '@kanade/api-types';
  import { createClient } from '@kanade/client';
  import { SvelteSet } from 'svelte/reactivity';
  import { Tabs, Toaster, weekStartLabel, type TabItem } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import { SURFACE_LABELS, actorName, describe, localAt, weekDate } from './describe';
  import RevertDialog from './RevertDialog.svelte';
  import { memberLabel } from '../names/directory.svelte';
  import ActorName from './ActorName.svelte';

  let { store, toaster }: { store: AdminWeek; toaster: Toaster } = $props();

  const client = createClient();
  let week = $state('');
  let actor = $state('');
  let records = $state<ChangeRecord[]>([]);
  let head = $state<HistoryPage['head'] | null>(null);
  let nextBefore = $state<number | null>(null);
  let total = $state(0);
  let error = $state('');
  let loading = $state(false);

  async function load(more = false) {
    loading = true;
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a request's query, built and sent, never state
    const params = new URLSearchParams({ limit: '20' });
    if (week) params.set('week', week);
    if (actor) params.set('actor', actor);
    if (more && nextBefore !== null) params.set('before', String(nextBefore));
    try {
      const page = await client.get<HistoryPage>(`/api/admin/history?${params}`);
      records = more ? [...records, ...page.records] : page.records;
      for (const r of page.records) if (r.actor.kind === 'admin') seenAdmins.add(r.actor.id);
      head = page.head;
      nextBefore = page.next_before;
      total = page.total;
      error = '';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not load the history.';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    void week;
    void actor;
    void load();
  });

  const checkpoints = new Resource<Checkpoints>('/api/admin/history/checkpoints');
  type Tab = 'timeline' | 'checkpoints';
  let tab = $state<Tab>('timeline');
  $effect(() => {
    if (tab === 'checkpoints') void checkpoints.load();
  });
  const tabs = $derived<TabItem<Tab>[]>([
    { id: 'timeline', label: 'Timeline', count: total },
    { id: 'checkpoints', label: 'Checkpoints' },
  ]);

  const names = (id: string) => memberLabel(store.members, id);
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');
  const weeks = $derived([...new Set(records.flatMap((r) => r.weeks))].sort().reverse());
  const groups = $derived(
    weeks.length
      ? weeks.map((w) => ({ week: w, records: records.filter((r) => r.weeks.includes(w)) }))
      : [{ week: '', records }],
  );
  const loose = $derived(records.filter((r) => r.weeks.length === 0));
  const members = $derived(store.members.filter((m) => m.bossing));
  const known = (id: string) => store.members.some((m) => m.id === id);
  // Admin ids as the history names them (`token`, `discord:<id>`, `tailscale:<login>`),
  // collected from every page read, so filtering by one keeps the others offered.
  const seenAdmins = new SvelteSet<string>();
  const admins = $derived(
    [...seenAdmins]
      .map((id) => ({ id, label: actorName({ kind: 'admin', id }, names, known) }))
      .sort((a, b) => a.label.localeCompare(b.label)),
  );

  // The dialog's request.
  let dialogOpen = $state(false);
  let dialog = $state<{ title: string; path: string; body: Record<string, unknown> }>({ title: '', path: '', body: {} });
  function revert(r: ChangeRecord) {
    dialog = { title: `Revert #${r.seq}?`, path: '/api/admin/history/revert', body: { seqs: [r.seq] } };
    dialogOpen = true;
  }
  // Records name a week by its starting instant; people read its guild-local start date.
  const weekLabel = (w: string) => weekStartLabel(weekDate(w, tz));
  function restore(w: string, r: ChangeRecord) {
    dialog = { title: `Restore the week of ${weekLabel(w)} to just after #${r.seq}?`, path: '/api/admin/history/restore-week', body: { week: w, revision: r.revision } };
    dialogOpen = true;
  }
  let who = $state('');
  let since = $state('');
  function revertMember(event: SubmitEvent) {
    event.preventDefault();
    if (!who) return;
    dialog = {
      title: `Revert everything ${names(who.split(':')[1] ?? '')} changed${since ? ` since ${since}` : ''}?`,
      path: '/api/admin/history/revert-actor',
      body: { actor: who, since: since ? `${since}T00:00:00` : '1970-01-01' },
    };
    dialogOpen = true;
  }

  async function done(plan: RevertPlan) {
    toaster.show({ message: plan.record ? `Reverted as #${plan.record.seq}.` : 'Nothing changed.', tone: 'ok' });
    await load();
    void store.refresh();
  }
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">Who changed what</p>
    <h1>{total.toLocaleString('en')} change{total === 1 ? '' : 's'}</h1>
    {#if head}<p class="note">Head #{head.seq} · <span class="mono">{head.hash.slice(0, 12)}</span></p>{/if}
  </div>
  <div class="filters history__filters" role="search" aria-label="Filter the history">
    <label class="field">
      <span>Week</span>
      <select bind:value={week}>
        <option value="">every week</option>
        {#if store.week}<option value={store.week.starts}>this boss week ({weekStartLabel(store.week.starts)})</option>{/if}
      </select>
    </label>
    <label class="field">
      <span>Who</span>
      <select bind:value={actor}>
        <option value="">everyone</option>
        {#each admins as a (a.id)}<option value="admin:{a.id}">{a.label}{a.id.startsWith('discord:') && known(a.id.slice(8)) ? ' (as admin)' : ''}</option>{/each}
        <option value="system:delivery">system (delivery)</option>
        {#each members as m (m.id)}<option value="member:{m.id}">{names(m.id)}</option>{/each}
      </select>
    </label>
  </div>
</div>

<Tabs items={tabs} bind:selected={tab} label="History">
  {#snippet panel(which)}
    {#if which === 'timeline'}
      <details class="history__member">
        <summary class="btn">Revert a member's changes…</summary>
        <form class="formrow" onsubmit={revertMember}>
          <label class="field">
            <span>Member</span>
            <select bind:value={who} required>
              <option value="">choose…</option>
              {#each members as m (m.id)}<option value="member:{m.id}">{names(m.id)}</option>{/each}
            </select>
          </label>
          <label class="field"><span>Since</span><input type="date" bind:value={since} /></label>
          <button class="btn" type="submit" disabled={!who}>Preview</button>
        </form>
      </details>
      {#if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
      {#snippet entry(r: ChangeRecord, w: string)}
        <li class="change" class:change--rollback={r.surface === 'rollback'}>
          <div class="change__head">
            <span class="change__seq mono">#{r.seq}</span>
            <strong><ActorName actor={r.actor} {names} {known} /></strong>
            <span class="chip chip--mono">{SURFACE_LABELS[r.surface] ?? r.surface}</span>
            {#if r.refs.length}<span class="chip">reverts {r.refs.map((x) => `#${x.seq}`).join(', ')}</span>{/if}
            <span class="id">{localAt(r.at, tz)}</span>
          </div>
          <ul class="change__lines">{#each describe(r, names, tz) as line, i (i)}<li>{line}</li>{/each}</ul>
          <details class="change__diff">
            <summary>Rows ({r.rows.length})</summary>
            <table>
              <caption class="vh">Rows changed by #{r.seq}</caption>
              <thead><tr><th scope="col">Row</th><th scope="col">Before</th><th scope="col">After</th></tr></thead>
              <tbody>
                {#each r.rows as row, i (i)}
                  <tr>
                    <th scope="row" class="mono">{'id' in row.key ? `${row.key.table}/${row.key.id}` : `rsvps/${row.key.run_id}/${row.key.user_id}`}</th>
                    <td><pre>{JSON.stringify(row.before, null, 1)}</pre></td>
                    <td><pre>{JSON.stringify(row.after, null, 1)}</pre></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </details>
          <div class="rowbtns">
            <button class="btn" type="button" onclick={() => revert(r)} aria-label="Revert #{r.seq}">Revert…</button>
            {#if w}
              <button class="btn btn--ghost" type="button" onclick={() => restore(w, r)} aria-label="Restore week {weekLabel(w)} to just after #{r.seq}">Restore week to here…</button>
            {/if}
          </div>
        </li>
      {/snippet}
      {#each groups as g (g.week)}
        {#if g.records.length}
          <section class="history__week" aria-labelledby="w-{weekDate(g.week, tz)}">
            <h3 class="pane__section" id="w-{weekDate(g.week, tz)}">{g.week ? `Boss week of ${weekLabel(g.week)}` : 'Changes'}</h3>
            <ol class="timeline">{#each g.records as r (r.seq)}{@render entry(r, g.week)}{/each}</ol>
          </section>
        {/if}
      {/each}
      {#if loose.length && weeks.length}
        <section aria-labelledby="w-none">
          <h3 class="pane__section" id="w-none">Weekly timings and other changes</h3>
          <ol class="timeline">{#each loose as r (r.seq)}{@render entry(r, '')}{/each}</ol>
        </section>
      {/if}
      {#if !loading && records.length === 0}<div class="empty"><strong>No changes match.</strong></div>{/if}
      {#if nextBefore !== null}
        <button class="btn" type="button" disabled={loading} onclick={() => void load(true)}>Older changes</button>
      {/if}
    {:else}
      {#if checkpoints.data}
        <p class="flash {checkpoints.data.verified.ok ? 'flash--ok' : 'flash--error'}" role="status">
          {checkpoints.data.verified.ok ? 'Chain verified' : 'Chain broken'}: {checkpoints.data.verified.checked} records, head
          #{checkpoints.data.verified.head.seq}.
        </p>
        <table>
          <caption class="vh">Backups anchoring the history</caption>
          <thead><tr><th scope="col">Backup</th><th scope="col">Taken</th><th scope="col">History head</th><th scope="col" class="num">Revision</th><th scope="col">Anchored</th></tr></thead>
          <tbody>
            {#each checkpoints.data.backups as b (b.file)}
              <tr>
                <th scope="row" class="mono">{b.file}</th>
                <td class="mono">{localAt(b.created_at, tz)}</td>
                <td class="mono">#{b.history_head.seq} · {b.history_head.hash.slice(0, 12)}</td>
                <td class="num">{b.revision}</td>
                <td>{b.anchored ? 'yes — a truncated history is refused' : 'no'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}<p class="note" aria-busy="true">Loading checkpoints…</p>{/if}
    {/if}
  {/snippet}
</Tabs>

<RevertDialog bind:open={dialogOpen} title={dialog.title} path={dialog.path} body={dialog.body} {names} timezone={tz} ondone={done} />

<style>
  .history__filters {
    margin: 0 0 0 auto;
  }

  .history__member {
    margin: 0.3rem 0 0.6rem;
  }

  .timeline {
    list-style: none;
    margin: 0;
    padding: 0 0 0 0.9rem;
    border-left: 3px solid var(--line);
  }

  .change {
    position: relative;
    padding: 0.5rem 0 0.8rem 0.6rem;
  }

  .change::before {
    content: '';
    position: absolute;
    left: -1.35rem;
    top: 0.85rem;
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 50%;
    background: var(--surface);
    border: 3px solid var(--win);
  }

  .change--rollback::before {
    border-color: var(--accent);
  }

  .change__head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.3rem 0.6rem;
  }

  .change__seq {
    font-weight: 600;
  }

  .change__lines {
    margin: 0.3rem 0;
    padding-left: 1.1rem;
  }

  .change__diff summary {
    cursor: pointer;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }

  .change__diff pre {
    font-size: var(--fs-mini);
    max-width: 22rem;
  }
</style>
