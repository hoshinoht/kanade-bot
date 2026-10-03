<!-- Immutable audit timeline: retain its recovery tools while making the
     selected change inspectable beside the list on wide screens. -->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/evidence.scss';
  import '@kanade/ui/styles/history.scss';
  import type { ChangeRecord, Checkpoints, HistoryPage, RevertPlan } from '@kanade/api-types';
  import { createClient } from '@kanade/client';
  import { SvelteSet } from 'svelte/reactivity';
  import { RowContent, Toaster, weekStartLabel } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import { SURFACE_LABELS, actorName, describe, localAt, weekDate } from './describe';
  import RevertDialog from './RevertDialog.svelte';
  import HistoryDetail from './HistoryDetail.svelte';
  import CheckpointsPanel from './CheckpointsPanel.svelte';
  import TextModal from '../shared/TextModal.svelte';
  import { memberLabel } from '../names/directory.svelte';

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
  let selectedSeq = $state<number | null>(null);
  let selectedWeek = $state('');
  let selectOnDesktop = $state(true);
  let wide = $state(false);
  let restore = '';
  let restoreGroup = '';

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
    selectedSeq = null;
    selectedWeek = '';
    selectOnDesktop = true;
    void load();
  });
  $effect(() => {
    const media = window.matchMedia('(min-width: 840px)');
    const update = () => (wide = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  // History opens the newest change only for an initial/filtered desktop view.
  // An explicitly closed pane stays closed until the user selects another row.
  $effect(() => {
    if (wide && selectOnDesktop && records.length) {
      const record = records[0];
      selectedSeq = record?.seq ?? null;
      selectedWeek = record?.weeks[0] ?? '';
      restore = String(record?.seq ?? '');
      restoreGroup = record?.weeks[0] ?? '';
      selectOnDesktop = false;
    }
  });

  const checkpoints = new Resource<Checkpoints>('/api/admin/history/checkpoints');
  type Tab = 'timeline' | 'checkpoints';
  const TABS: { id: Tab; label: string }[] = [
    { id: 'timeline', label: 'Timeline' },
    { id: 'checkpoints', label: 'Checkpoints' },
  ];
  let tab = $state<Tab>('timeline');
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = TABS[(target + TABS.length) % TABS.length]!;
    tab = next.id;
    document.getElementById(`history-tab-${next.id}`)?.focus({ preventScroll: true });
  }
  const names = (id: string) => memberLabel(store.members, id);
  const tz = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');
  const weeks = $derived([...new Set(records.flatMap((r) => r.weeks))].sort().reverse());
  const groups = $derived(weeks.length ? weeks.map((w) => ({ week: w, records: records.filter((r) => r.weeks.includes(w)) })) : [{ week: '', records }]);
  const loose = $derived(records.filter((r) => r.weeks.length === 0));
  const members = $derived(store.members.filter((m) => m.bossing));
  const known = (id: string) => store.members.some((m) => m.id === id);
  const seenAdmins = new SvelteSet<string>();
  const admins = $derived([...seenAdmins].map((id) => ({ id, label: actorName({ kind: 'admin', id }, names, known) })).sort((a, b) => a.label.localeCompare(b.label)));
  const selected = $derived(records.find((record) => record.seq === selectedSeq) ?? null);
  // A change spanning several boss weeks is listed once per week: only the row
  // that was opened is active.
  const active = (record: ChangeRecord, group: string) => record.seq === selectedSeq && group === selectedWeek;

  let dialogOpen = $state(false);
  let dialog = $state<{ title: string; path: string; body: Record<string, unknown> }>({ title: '', path: '', body: {} });
  function revert(record: ChangeRecord) {
    dialog = { title: `Revert #${record.seq}?`, path: '/api/admin/history/revert', body: { seqs: [record.seq] } };
    dialogOpen = true;
  }
  const weekLabel = (value: string) => weekStartLabel(weekDate(value, tz));
  function restoreWeek(value: string, record: ChangeRecord) {
    dialog = { title: `Restore the week of ${weekLabel(value)} to just after #${record.seq}?`, path: '/api/admin/history/restore-week', body: { week: value, revision: record.revision } };
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
  function open(record: ChangeRecord, event: MouseEvent, openedWeek: string) {
    restore = String(record.seq);
    restoreGroup = openedWeek;
    selectedSeq = record.seq;
    selectedWeek = openedWeek;
    selectOnDesktop = false;
    (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true });
  }
  function closeDetail() {
    selectedSeq = null;
    selectedWeek = '';
    selectOnDesktop = false;
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-history="${restore}"][data-history-week="${restoreGroup}"]`)?.focus({ preventScroll: true }));
  }
  // The raw record opens in the shared long-text viewer (as on the Chat turn
  // page), outside the list/pane row so the two stay the body's only children.
  let raw = $state({ open: false, title: '', text: '' });
  const showRaw = (record: ChangeRecord) => (raw = { open: true, title: `Change #${record.seq} raw JSON`, text: JSON.stringify(record, null, 2) });
  async function done(plan: RevertPlan) {
    toaster.show({ message: plan.record ? `Reverted as #${plan.record.seq}.` : 'Nothing changed.', tone: 'ok' });
    await load();
    void store.refresh();
  }
</script>

<!-- One timeline row (B_History `.ev`): a dot, then the facts line over the summary. -->
{#snippet rowBody(record: ChangeRecord, isActive: boolean)}
  {@const lines = describe(record, names, tz)}
  {@const count = `${record.rows.length} row${record.rows.length === 1 ? '' : 's'}`}
  <span class="history-row__dot" aria-hidden="true"></span>
  <RowContent expanded={isActive}>
    {#snippet compact()}<span class="mono history-row__seq">#{record.seq}</span> <strong class="history-row__actor">{actorName(record.actor, names, known)}</strong> · <span class="history-row__summary">{lines.length ? lines.join(' · ') : `${count} changed`}</span> · {SURFACE_LABELS[record.surface] ?? record.surface} · {localAt(record.at, tz)} · {count}{#if record.refs.length} · reverts {record.refs.map((ref) => `#${ref.seq}`).join(', ')}{/if}{/snippet}
    <span class="history-row__text">
    <span class="history-row__head"><span class="mono history-row__seq">#{record.seq}</span><strong class="history-row__actor">{actorName(record.actor, names, known)}</strong><span class="chip chip--mono">{SURFACE_LABELS[record.surface] ?? record.surface}</span>{#if record.refs.length}<span class="chip">reverts {record.refs.map((ref) => `#${ref.seq}`).join(', ')}</span>{/if}<span class="history-row__time mono">{localAt(record.at, tz)}</span><span class="history-row__rows mono">{count}</span>{#if isActive}<span class="history-row__open cap">open</span>{/if}</span>
    <span class="history-row__summary">{lines.length ? lines.join(' · ') : `${count} changed`}</span>
    </span>
  </RowContent>
{/snippet}

<!-- Revert everything one member changed (B_History: the box at the foot of the change pane). -->
{#snippet memberRevert()}
  <form class="history-member" data-fid="history-revert-member" onsubmit={revertMember}>
    <h3 class="cap">Revert a member's changes…</h3>
    <div class="history-member__fields">
      <label class="history-member__who"><span class="vh">Member</span><select bind:value={who} required><option value="">choose…</option>{#each members as member (member.id)}<option value="member:{member.id}">{names(member.id)}</option>{/each}</select></label>
      <label class="history-member__since"><span class="vh">Since</span><input class="mono" type="date" bind:value={since} /></label>
    </div>
    <button class="btn" type="submit" disabled={!who}>Preview</button>
  </form>
{/snippet}

<PageLine title="History">
  <h1><span class="pageline__num">{total.toLocaleString('en')}</span> change{total === 1 ? '' : 's'}</h1>
  {#if head}<p class="pageline__context">head #{head.seq} · <span class="mono">{head.hash.slice(0, 12)}</span></p>{/if}
</PageLine>

<section data-fid="window" class="card history-window window-fill" aria-labelledby="history-title">
  <header class="card__head tabs__strip history-window__head" data-fid="window-bar">
    <h2 class="vh" id="history-title">History</h2>
    <div class="tabs__tabs" role="tablist" aria-label="History" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        <button class="tabs__tab" role="tab" type="button" id="history-tab-{t.id}" aria-selected={tab === t.id} aria-controls="history-{t.id}" tabindex={tab === t.id ? 0 : -1} onclick={() => (tab = t.id)} onkeydown={(event) => tabKey(event, index)}>{t.label}{#if t.id === 'timeline'}<span class="tabs__count">{total}</span>{:else if checkpoints.data}<span class="tabs__count">{checkpoints.data.backups.length}</span>{/if}</button>
      {/each}
    </div>
    <!-- The filters only apply to the Timeline. -->
    <div class="history-window__filters" data-fid="window-filters" hidden={tab !== 'timeline'} role="search" aria-label="Filter the history">
      <label class="btn history-filter"><span class="history-filter__label">Week</span><select bind:value={week}><option value="">every week</option>{#if store.week}<option value={store.week.starts}>this boss week ({weekStartLabel(store.week.starts)})</option>{/if}</select></label>
      <label class="btn history-filter"><span class="history-filter__label">Who</span><select bind:value={actor}><option value="">everyone</option>{#each admins as admin (admin.id)}<option value="admin:{admin.id}">{admin.label}{admin.id.startsWith('discord:') && known(admin.id.slice(8)) ? ' (as admin)' : ''}</option>{/each}<option value="system:delivery">system (delivery)</option>{#each members as member (member.id)}<option value="member:{member.id}">{names(member.id)}</option>{/each}</select></label>
    </div>
  </header>

  {#if tab === 'timeline'}
    <div class="history-window__body" id="history-timeline" role="tabpanel" aria-labelledby="history-tab-timeline">
      <div class="history-list-region">
        <div class="history-list-region__scroll" data-fid="history-list">
          <!-- B_History: with a change open on a wide screen, this tool sits at the foot of its pane. -->
          {#if !(wide && selected)}
            <details class="history__member">
              <summary class="btn">Revert a member's changes…</summary>
              <form class="formrow" onsubmit={revertMember}>
                <label class="field"><span>Member</span><select bind:value={who} required><option value="">choose…</option>{#each members as member (member.id)}<option value="member:{member.id}">{names(member.id)}</option>{/each}</select></label>
                <label class="field"><span>Since</span><input type="date" bind:value={since} /></label>
                <button class="btn" type="submit" disabled={!who}>Preview</button>
              </form>
            </details>
          {/if}
          {#if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
          {#each groups as group (group.week)}
            {#if group.records.length}
              <section class="history__week" aria-labelledby="history-week-{weekDate(group.week, tz)}">
                <h3 class="pane__section" data-fid="history-group" id="history-week-{weekDate(group.week, tz)}">{group.week ? `Boss week of ${weekLabel(group.week)}` : 'Changes'}</h3>
                <ol class="history-timeline">
                  {#each group.records as record (record.seq)}
                    <li>
                       <button class="history-row expandable-row" data-fid="history-row" class:history-row--active={active(record, group.week)} type="button" aria-current={active(record, group.week) ? 'true' : undefined} data-history={record.seq} data-history-week={group.week} onclick={(event) => open(record, event, group.week)}>
                        {@render rowBody(record, active(record, group.week))}
                      </button>
                    </li>
                  {/each}
                </ol>
              </section>
            {/if}
          {/each}
           {#if loose.length && weeks.length}<section class="history__week" aria-labelledby="history-week-none"><h3 class="pane__section" data-fid="history-group" id="history-week-none">Weekly timings and other changes</h3><ol class="history-timeline">{#each loose as record (record.seq)}<li><button class="history-row expandable-row" class:history-row--active={active(record, '')} type="button" aria-current={active(record, '') ? 'true' : undefined} data-history={record.seq} data-history-week="" onclick={(event) => open(record, event, '')}>{@render rowBody(record, active(record, ''))}</button></li>{/each}</ol></section>{/if}
          {#if !loading && records.length === 0}<div class="empty"><strong>No changes match.</strong></div>{/if}
        </div>
        {#if nextBefore !== null}<div class="history-list-region__pager"><button class="btn" type="button" disabled={loading} onclick={() => void load(true)}>Older changes</button></div>{/if}
      </div>
      {#if selected}<HistoryDetail wide={wide} member={memberRevert} record={selected} week={selectedWeek} timezone={tz} {names} onclose={closeDetail} onrevert={revert} onrestore={restoreWeek} onraw={showRaw} />{/if}
    </div>
  {:else}
    <div class="history-window__body" id="history-checkpoints" role="tabpanel" aria-labelledby="history-tab-checkpoints">
      <div class="history-list-region"><CheckpointsPanel {checkpoints} timezone={tz} /></div>
    </div>
  {/if}
</section>

<RevertDialog bind:open={dialogOpen} title={dialog.title} path={dialog.path} body={dialog.body} {names} timezone={tz} ondone={done} returnFocus={() => document.querySelector<HTMLElement>(`[data-history-revert="${selectedSeq ?? restore}"]`) ?? document.querySelector<HTMLElement>(`[data-history="${selectedSeq ?? restore}"]`)} />
<TextModal bind:open={raw.open} title={raw.title} eyebrow="History" text={raw.text} {toaster} />
