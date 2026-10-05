<!-- Immutable audit timeline: retain its recovery tools while making the
     selected change inspectable beside the list on wide screens. -->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/evidence.scss';
  import '@kanade/ui/styles/history.scss';
  import '@kanade/ui/styles/date-picker.scss';
  import '@kanade/ui/styles/select.scss';
  import type { ChangeRecord, Checkpoints, HistoryPage, RevertPlan, SettingsChangeRow } from '@kanade/api-types';
  import { createClient } from '@kanade/client';
  import { SvelteSet } from 'svelte/reactivity';
  import { onMount } from 'svelte';
  import { DatePicker, Icon, Presence, RowContent, Select, serverClock, Toaster, TWO_PANE_QUERY, weekStartLabel, type SelectOption } from '@kanade/ui';
  import { Resource } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import { SURFACE_LABELS, actorName, describe, localAt, weekDate } from './describe';
  import RevertDialog from './RevertDialog.svelte';
  import HistoryDetail from './HistoryDetail.svelte';
  import ConfigDetail from './ConfigDetail.svelte';
  import { mergeTimeline, sectionLabel, settingCount, settingSummary, type TimelineItem } from './settings';
  import CheckpointsPanel from './CheckpointsPanel.svelte';
  import TextModal from '../shared/TextModal.svelte';
  import { memberLabel } from '../names/directory.svelte';

  let { store, toaster }: { store: AdminWeek; toaster: Toaster } = $props();
  const client = createClient();
  let week = $state('');
  let actor = $state('');
  let records = $state<ChangeRecord[]>([]);
  // Config section saves: view-only rows interleaved by time (not in the chain).
  let settings = $state<SettingsChangeRow[]>([]);
  let settingsTotal = $state(0);
  let head = $state<HistoryPage['head'] | null>(null);
  let nextBefore = $state<number | null>(null);
  let total = $state(0);
  let error = $state('');
  let loading = $state(false);
  let selectedSeq = $state<number | null>(null);
  let selectedConfig = $state<number | null>(null);
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
      // A clock step back at a page boundary can repeat a save: keep each id once.
      settings = more ? [...settings, ...page.settings.filter((s) => !settings.some((had) => had.id === s.id))] : page.settings;
      for (const r of [...page.records, ...page.settings]) if (r.actor.kind === 'admin') seenAdmins.add(r.actor.id);
      head = page.head;
      nextBefore = page.next_before;
      total = page.total;
      settingsTotal = page.settings_total;
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
    selectedConfig = null;
    selectedWeek = '';
    selectOnDesktop = true;
    void load();
  });
  $effect(() => {
    const media = window.matchMedia(TWO_PANE_QUERY);
    const update = () => (wide = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });
  // History opens the newest change only for an initial/filtered desktop view.
  // An explicitly closed pane stays closed until the user selects another row.
  $effect(() => {
    const first = timeline[0];
    if (wide && selectOnDesktop && first) {
      const week = first.kind === 'change' ? (first.record.weeks[0] ?? '') : first.change.week;
      if (first.kind === 'change') selectedSeq = first.record.seq;
      else selectedConfig = first.change.id;
      selectedWeek = week;
      restore = itemKey(first);
      restoreGroup = week;
      selectOnDesktop = false;
    }
  });

  const checkpoints = new Resource<Checkpoints>('/api/admin/history/checkpoints');
  // The rail's tags: a backup snapshot sits on the record its manifest names as
  // head (same hash, so a backup the chain no longer holds tags nothing), and a
  // record a loaded rollback undid points back at that rollback.
  onMount(() => void checkpoints.load());
  const snapshots = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup rebuilt by the derivation, never mutated after
    const byHash = new Map<string, Checkpoints['backups']>();
    for (const backup of checkpoints.data?.backups ?? []) byHash.set(backup.history_head.hash, [...(byHash.get(backup.history_head.hash) ?? []), backup]);
    return byHash;
  });
  const revertedBy = $derived.by(() => {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a lookup rebuilt by the derivation, never mutated after
    const by = new Map<number, number[]>();
    for (const record of records) for (const ref of record.refs) by.set(ref.seq, [...(by.get(ref.seq) ?? []), record.seq]);
    return by;
  });
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
  const timeline = $derived(mergeTimeline(records, settings));
  const itemKey = (item: TimelineItem) => (item.kind === 'change' ? String(item.record.seq) : `c${item.change.id}`);
  const weeks = $derived([...new Set([...records.flatMap((r) => r.weeks), ...settings.map((s) => s.week)])].sort().reverse());
  const groups = $derived(weeks.length ? weeks.map((w) => ({ week: w, items: timeline.filter((item) => (item.kind === 'change' ? item.record.weeks.includes(w) : item.change.week === w)) })) : [{ week: '', items: timeline }]);
  const loose = $derived(records.filter((r) => r.weeks.length === 0));
  const members = $derived(store.members.filter((m) => m.bossing));
  const known = (id: string) => store.members.some((m) => m.id === id);
  const seenAdmins = new SvelteSet<string>();
  const admins = $derived([...seenAdmins].map((id) => ({ id, label: actorName({ kind: 'admin', id }, names, known) })).sort((a, b) => a.label.localeCompare(b.label)));
  const memberOption = (m: { id: string; name: string }, value: string, group?: string): SelectOption => {
    const label = names(m.id);
    return { value, label, group, mono: label.slice(0, 1).toUpperCase(), keywords: m.name };
  };
  const weekOptions = $derived<SelectOption[]>([
    { value: '', label: 'every week' },
    ...(store.week ? [{ value: store.week.starts, label: 'this boss week', sub: weekStartLabel(store.week.starts) }] : []),
  ]);
  const actorOptions = $derived<SelectOption[]>([
    { value: '', label: 'everyone', icon: 'users' },
    ...admins.map((a) => ({
      value: `admin:${a.id}`,
      label: a.label,
      sub: a.id.startsWith('discord:') && known(a.id.slice(8)) ? 'as admin' : undefined,
      group: 'Admins',
      mono: a.label.slice(0, 1).toUpperCase(),
    })),
    { value: 'system:delivery', label: 'system', sub: 'delivery', group: 'System', icon: 'sliders' },
    ...members.map((m) => memberOption(m, `member:${m.id}`, 'Members')),
  ]);
  const whoOptions = $derived<SelectOption[]>(members.map((m) => memberOption(m, `member:${m.id}`)));
  const selected = $derived(records.find((record) => record.seq === selectedSeq) ?? null);
  const selectedSave = $derived(settings.find((change) => change.id === selectedConfig) ?? null);
  // The pane outlives the selection by its exit animation; while open it reads
  // the live record (the presence copy lags an effect behind).
  type Shown = { kind: 'change'; record: ChangeRecord; week: string } | { kind: 'config'; change: SettingsChangeRow };
  const pane = new Presence<Shown>();
  $effect(() => pane.set(selected ? { kind: 'change', record: selected, week: selectedWeek } : selectedSave ? { kind: 'config', change: selectedSave } : null));
  // A change spanning several boss weeks is listed once per week: only the row
  // that was opened is active.
  const active = (record: ChangeRecord, group: string) => record.seq === selectedSeq && group === selectedWeek;
  const activeSave = (change: SettingsChangeRow) => change.id === selectedConfig;

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
  // "Since" is the start of a guild-local day; today and boss weeks are the server's.
  const clock = $derived(serverClock(store.week));
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
    selectedConfig = null;
    selectedWeek = openedWeek;
    selectOnDesktop = false;
    (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true });
  }
  function openSave(change: SettingsChangeRow, event: MouseEvent) {
    restore = `c${change.id}`;
    restoreGroup = change.week;
    selectedConfig = change.id;
    selectedSeq = null;
    selectedWeek = change.week;
    selectOnDesktop = false;
    (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true });
  }
  function closeDetail() {
    selectedSeq = null;
    selectedConfig = null;
    selectedWeek = '';
    selectOnDesktop = false;
    requestAnimationFrame(() => document.querySelector<HTMLButtonElement>(`[data-history="${restore}"][data-history-week="${restoreGroup}"]`)?.focus({ preventScroll: true }));
  }
  // The raw record opens in the shared long-text viewer (as on the Chat turn
  // page), outside the list/pane row so the two stay the body's only children.
  let raw = $state({ open: false, title: '', text: '' });
  const showRaw = (record: ChangeRecord) => (raw = { open: true, title: `Change #${record.seq} raw JSON`, text: JSON.stringify(record, null, 2) });
  const showSaveRaw = (change: SettingsChangeRow) => (raw = { open: true, title: `${sectionLabel(change.section)} settings raw JSON`, text: JSON.stringify(change, null, 2) });
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
  {@const backups = snapshots.get(record.hash) ?? []}
  {@const undoneBy = revertedBy.get(record.seq) ?? []}
  <span class="history-row__dot" class:history-row__dot--revert={record.refs.length} class:history-row__dot--undone={undoneBy.length} aria-hidden="true"></span>
  <RowContent expanded={isActive}>
    {#snippet compact()}<span class="mono history-row__seq">#{record.seq}</span> <strong class="history-row__actor">{actorName(record.actor, names, known)}</strong> · <span class="history-row__summary">{lines.length ? lines.join(' · ') : `${count} changed`}</span> · {SURFACE_LABELS[record.surface] ?? record.surface} · {localAt(record.at, tz)} · {count}{/snippet}
    <span class="history-row__text">
    <span class="history-row__head"><span class="mono history-row__seq">#{record.seq}</span><strong class="history-row__actor">{actorName(record.actor, names, known)}</strong><span class="chip chip--mono">{SURFACE_LABELS[record.surface] ?? record.surface}</span><span class="history-row__time mono">{localAt(record.at, tz)}</span><span class="history-row__rows mono">{count}</span>{#if isActive}<span class="history-row__open cap">open</span>{/if}</span>
    <span class="history-row__summary">{lines.length ? lines.join(' · ') : `${count} changed`}</span>
    </span>
  </RowContent>
  <!-- Tags on the rail (always shown, not only on the opened row): a rollback names what it undid, the undone record names its rollback, a backup marks its snapshot. -->
  {#if record.refs.length || undoneBy.length || backups.length}
    <span class="history-row__tags">
      {#if record.refs.length}<span class="history-tag history-tag--revert"><Icon name="rotate-ccw" />reverts {record.refs.map((ref) => `#${ref.seq}`).join(', ')}</span>{/if}
      {#if undoneBy.length}<span class="history-tag history-tag--undone">reverted by {undoneBy.map((seq) => `#${seq}`).join(', ')}</span>{/if}
      {#each backups as backup (backup.file)}<span class="history-tag history-tag--backup" title="{backup.file} · taken {localAt(backup.created_at, tz)}"><Icon name="pin" />backup<span class="vh"> {backup.file}, taken {localAt(backup.created_at, tz)}</span></span>{/each}
    </span>
  {/if}
{/snippet}

<!-- A Config save: the same row shape, a square dot and a "Config" chip; view-only. -->
{#snippet saveBody(change: SettingsChangeRow, isActive: boolean)}
  {@const who = actorName(change.actor, names, known)}
  <span class="history-row__dot history-row__dot--config" aria-hidden="true"></span>
  <RowContent expanded={isActive}>
    {#snippet compact()}<strong class="history-row__actor">{who}</strong> · <span class="history-row__summary">{settingSummary(change)}</span> · {SURFACE_LABELS[change.surface] ?? change.surface} · {localAt(change.at, tz)} · {settingCount(change)}{/snippet}
    <span class="history-row__text">
    <span class="history-row__head"><strong class="history-row__actor">{who}</strong><span class="chip chip--mono history-row__config">Config</span><span class="chip chip--mono">{SURFACE_LABELS[change.surface] ?? change.surface}</span><span class="history-row__time mono">{localAt(change.at, tz)}</span><span class="history-row__rows mono">{settingCount(change)}</span>{#if isActive}<span class="history-row__open cap">open</span>{/if}</span>
    <span class="history-row__summary">{settingSummary(change)}</span>
    </span>
  </RowContent>
{/snippet}

<!-- Revert everything one member changed (B_History: the box at the foot of the change pane). -->
{#snippet memberRevert()}
  <form class="history-member" data-fid="history-revert-member" onsubmit={revertMember}>
    <h3 class="cap">Revert a member's changes…</h3>
    <div class="history-member__fields">
      <div class="history-member__who"><Select label="Member" options={whoOptions} bind:value={who} placeholder="choose…" noun="members" /></div>
      <div class="history-member__since"><DatePicker mode="single" label="Since" lead="Changes since" {clock} value={since} onpick={(day) => (since = day)} /></div>
    </div>
    <button class="btn" type="submit" disabled={!who}>Preview</button>
  </form>
{/snippet}

<PageLine title="History">
  <h1><span class="pageline__num">{(total + settingsTotal).toLocaleString('en')}</span> change{total + settingsTotal === 1 ? '' : 's'}</h1>
  {#if head}<p class="pageline__context">head #{head.seq} · <span class="mono">{head.hash.slice(0, 12)}</span></p>{/if}
</PageLine>

<section data-fid="window" class="card history-window window-fill" aria-labelledby="history-title">
  <header class="card__head tabs__strip history-window__head" data-fid="window-bar">
    <h2 class="vh" id="history-title">History</h2>
    <div class="tabs__tabs" role="tablist" aria-label="History" data-fid="window-tabs">
      {#each TABS as t, index (t.id)}
        <button class="tabs__tab" role="tab" type="button" id="history-tab-{t.id}" aria-selected={tab === t.id} aria-controls="history-{t.id}" tabindex={tab === t.id ? 0 : -1} onclick={() => (tab = t.id)} onkeydown={(event) => tabKey(event, index)}>{t.label}{#if t.id === 'timeline'}<span class="tabs__count">{total + settingsTotal}</span>{:else if checkpoints.data}<span class="tabs__count">{checkpoints.data.backups.length}</span>{/if}</button>
      {/each}
    </div>
    <!-- The filters only apply to the Timeline. -->
    <div class="history-window__filters" data-fid="window-filters" hidden={tab !== 'timeline'} role="search" aria-label="Filter the history">
      <Select size="tbar" label="Week" options={weekOptions} bind:value={week} />
      <Select size="tbar" label="Who" options={actorOptions} bind:value={actor} noun="people" />
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
                <div class="field"><span>Member</span><Select label="Member" options={whoOptions} bind:value={who} placeholder="choose…" noun="members" /></div>
                <div class="field"><DatePicker mode="single" label="Since" lead="Changes since" {clock} value={since} onpick={(day) => (since = day)} /></div>
                <button class="btn" type="submit" disabled={!who}>Preview</button>
              </form>
            </details>
          {/if}
          {#if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
          {#each groups as group (group.week)}
            {#if group.items.length}
              <section class="history__week" aria-labelledby="history-week-{weekDate(group.week, tz)}">
                <h3 class="pane__section" data-fid="history-group" id="history-week-{weekDate(group.week, tz)}">{group.week ? `Boss week of ${weekLabel(group.week)}` : 'Changes'}</h3>
                <ol class="history-timeline">
                  {#each group.items as item (itemKey(item))}
                    <li>
                      {#if item.kind === 'change'}
                        {@const record = item.record}
                        <button class="history-row expandable-row" data-fid="history-row" class:history-row--active={active(record, group.week)} type="button" aria-current={active(record, group.week) ? 'true' : undefined} data-history={record.seq} data-history-week={group.week} onclick={(event) => open(record, event, group.week)}>
                          {@render rowBody(record, active(record, group.week))}
                        </button>
                      {:else}
                        {@const change = item.change}
                        <button class="history-row history-row--config expandable-row" data-fid="history-row" class:history-row--active={activeSave(change)} type="button" aria-current={activeSave(change) ? 'true' : undefined} data-history="c{change.id}" data-history-week={group.week} onclick={(event) => openSave(change, event)}>
                          {@render saveBody(change, activeSave(change))}
                        </button>
                      {/if}
                    </li>
                  {/each}
                </ol>
              </section>
            {/if}
          {/each}
           {#if loose.length && weeks.length}<section class="history__week" aria-labelledby="history-week-none"><h3 class="pane__section" data-fid="history-group" id="history-week-none">Weekly timings and other changes</h3><ol class="history-timeline">{#each loose as record (record.seq)}<li><button class="history-row expandable-row" class:history-row--active={active(record, '')} type="button" aria-current={active(record, '') ? 'true' : undefined} data-history={record.seq} data-history-week="" onclick={(event) => open(record, event, '')}>{@render rowBody(record, active(record, ''))}</button></li>{/each}</ol></section>{/if}
          {#if !loading && records.length === 0 && settings.length === 0}<div class="empty"><strong>No changes match.</strong></div>{/if}
        </div>
        {#if nextBefore !== null}<div class="history-list-region__pager"><button class="btn" type="button" disabled={loading} onclick={() => void load(true)}>Older changes</button></div>{/if}
      </div>
      {#if pane.shown?.kind === 'change'}<HistoryDetail wide={wide} member={memberRevert} record={selected ?? pane.shown.record} week={selected ? selectedWeek : pane.shown.week} timezone={tz} {names} onclose={closeDetail} onrevert={revert} onrestore={restoreWeek} onraw={showRaw} leaving={pane.leaving} onleft={(event) => pane.done(event)} />
      {:else if pane.shown?.kind === 'config'}{@const change = selectedSave ?? pane.shown.change}<ConfigDetail wide={wide} {change} timezone={tz} actor={actorName(change.actor, names, known)} onclose={closeDetail} onraw={showSaveRaw} leaving={pane.leaving} onleft={(event) => pane.done(event)} />{/if}
    </div>
  {:else}
    <div class="history-window__body" id="history-checkpoints" role="tabpanel" aria-labelledby="history-tab-checkpoints">
      <div class="history-list-region"><CheckpointsPanel {checkpoints} timezone={tz} /></div>
    </div>
  {/if}
</section>

<RevertDialog bind:open={dialogOpen} title={dialog.title} path={dialog.path} body={dialog.body} {names} timezone={tz} ondone={done} returnFocus={() => document.querySelector<HTMLElement>(`[data-history-revert="${selectedSeq ?? restore}"]`) ?? document.querySelector<HTMLElement>(`[data-history="${selectedSeq ?? restore}"]`)} />
<TextModal bind:open={raw.open} title={raw.title} eyebrow="History" text={raw.text} {toaster} />
