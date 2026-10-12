<!--
  History's Graph tab (boards B_HistoryGraph*, B_PhoneHistoryGraph*): one
  table row per change, newest first, with the admin's own drafts as lanes
  beside them and the open drafts' tips above the head. The table is the
  graph: the lane cell is aria-hidden and the Drafts column (on phones, the
  row's last line) says every branch the lanes draw. The keyboard model is a
  grid with one tab stop: ↑/↓ rows, Home/End, → into the row's buttons and ←
  back, Enter opens, Escape closes the pane. Pages like the Timeline (20 a
  read); quiet stretches fold; Go to # windows the view around a change.
-->
<script lang="ts">
  import '@kanade/ui/styles/screens/drafts.scss';
  import '@kanade/ui/styles/screens/inbox.scss';
  import '@kanade/ui/styles/screens/history-graph.scss';
  import type { ChangeRecord, DraftDetail, DraftGraph, DraftPreviewView, GraphDraft, HistoryPage, Run, Week } from '@kanade/api-types';
  import { createClient } from '@kanade/client';
  import { DecisionCard, Icon, LoadError, LoadingState, Modal, StateNote, StatusChip, runTitle } from '@kanade/ui';
  import { onMount, tick } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { live } from '../resource.svelte';
  import { draftPath, removalLabel, shortDate } from '../drafts/drafts';
  import { SURFACE_LABELS, actorName, describe, localAt, runTitles, shortAt, type Names } from './describe';
  import { closeWord, draftHref, draftsAt, focusable, laneCount, layout, tipLine, type Chip, type Row } from './graph';
  import GraphLanes from './GraphLanes.svelte';
  import HistoryDetail from './HistoryDetail.svelte';

  let {
    names,
    known,
    timezone,
    week,
    runs,
    wide,
    view,
    onrevert,
    onrestore,
    onraw,
    ontimeline,
    onwindow,
    onempty,
  }: {
    names: Names;
    known: (id: string) => boolean;
    timezone: string;
    week: Week | null;
    /** The runs of the weeks the app holds, to name an answer's run (records name it by id). */
    runs: Run[];
    /** Two panes: the side pane and the Drafts column; else the sheet and chips in the row. */
    wide: boolean;
    view: 'graph' | 'list';
    onrevert: (record: ChangeRecord) => void;
    onrestore: (week: string, record: ChangeRecord) => void;
    onraw: (record: ChangeRecord) => void;
    /** "Open Timeline" from the empty state. */
    ontimeline: () => void;
    /** The change Go to # windowed the view around (`null`: back at the head), for the title bar's field. */
    onwindow?: (target: number | null) => void;
    /** Whether the empty state shows (the phone's title tools give way to it, B_PhoneHistoryGraphEmpty). */
    onempty?: (empty: boolean) => void;
  } = $props();

  const client = createClient();
  const PAGE = 20;
  /** Go to #n shows #n and the ten changes above it. */
  const WINDOW = 11;
  const CAPTION =
    'History graph. Each row is one History change, newest first, or an open draft above the head. The Drafts column states every branch the lanes draw: where a draft starts, merges, or is discarded or expired.';

  let records = $state<ChangeRecord[]>([]);
  let head = $state<HistoryPage['head'] | null>(null);
  let nextBefore = $state<number | null>(null);
  let drafts = $state<GraphDraft[]>([]);
  let loaded = $state(false);
  let loading = $state(false);
  let error = $state('');
  /** The change Go to # windowed the view around; `null` at the head. */
  let target = $state<number | null>(null);
  let tipsOpen = $state(false);
  const expanded = new SvelteSet<string>();
  // Conflicts are counted only for drafts opened here (their preview), never for the whole graph.
  let conflicts = $state<Record<string, number>>({});
  let previews = $state<Record<string, DraftPreviewView>>({});
  let selected = $state<{ kind: 'change'; seq: number } | { kind: 'draft'; id: string } | null>(null);
  let focusKey = $state('');
  let tableEl = $state<HTMLTableElement>();

  const headSeq = $derived(head?.seq ?? 0);
  const selectedSeq = $derived(selected?.kind === 'change' ? selected.seq : null);
  const selectedDraft = $derived(selected?.kind === 'draft' ? selected.id : '');
  const rows = $derived(
    layout({
      records,
      head: headSeq,
      drafts,
      more: nextBefore !== null,
      conflicts,
      expanded,
      tipsCollapsed: target !== null && !tipsOpen,
      keep: new Set([selectedSeq, target].filter((seq) => seq !== null)),
    }),
  );
  const reachable = $derived(focusable(rows));
  const selectedKey = $derived(selectedSeq !== null ? `r${selectedSeq}` : selectedDraft ? (reachable.find((r) => r.key === `t-${selectedDraft}` || r.key === `e-${selectedDraft}`)?.key ?? '') : '');
  // The roving tab stop: the focused row, else the selection, else the first row.
  const stop = $derived(reachable.some((r) => r.key === focusKey) ? focusKey : reachable.some((r) => r.key === selectedKey) ? selectedKey : (reachable[0]?.key ?? ''));
  // Lanes 24 px apart on wide screens, 16 px on phones; the column is as wide as the lanes drawn (the History line alone with no drafts).
  const lanes = $derived(laneCount(rows));
  const x = $derived((lane: number) => (wide ? 14 + lane * 24 : 10 + lane * 16));
  const width = $derived(wide ? 28 + lanes * 24 : 18 + lanes * 16);
  const titles = $derived(runTitles(runs, records));
  // Nothing to draw anywhere: no lanes in the loaded records and nothing older to load.
  const empty = $derived(loaded && !error && drafts.length === 0 && nextBefore === null && target === null);
  $effect(() => onempty?.(empty));
  const selectedRecord = $derived(selectedSeq !== null ? (records.find((r) => r.seq === selectedSeq) ?? null) : null);
  const openCount = $derived(drafts.filter((d) => d.status === 'open').length);
  const label = $derived(
    records.length
      ? `History graph, changes #${records.at(-1)!.seq} to #${records[0]!.seq}${target !== null ? '; newer changes folded above' : ''}${openCount ? ` and ${openCount} open draft${openCount === 1 ? '' : 's'}` : ''}`
      : 'History graph',
  );

  async function page(before: number | null, limit: number): Promise<HistoryPage> {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a request's query, built and sent, never state
    const params = new URLSearchParams({ limit: String(limit) });
    if (before !== null) params.set('before', String(before));
    return client.get<HistoryPage>(`/api/admin/history?${params}`);
  }

  /** The lanes for the loaded records: own drafts spanning them, any status. */
  async function readDrafts() {
    if (!head) return;
    const from = records.at(-1)?.seq ?? head.seq;
    drafts = (await client.get<DraftGraph>(`/api/admin/drafts/graph?from=${from}&to=${head.seq}`)).drafts;
  }

  async function run(work: () => Promise<void>) {
    loading = true;
    try {
      await work();
      await readDrafts();
      error = '';
      loaded = true;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not load the graph.';
    } finally {
      loading = false;
    }
  }

  function leaveWindow() {
    target = null;
    onwindow?.(null);
  }

  /** The newest page, back at the head. */
  function load() {
    return run(async () => {
      const read = await page(null, PAGE);
      records = read.records;
      head = read.head;
      nextBefore = read.next_before;
      leaveWindow();
    });
  }

  function older() {
    const before = nextBefore;
    if (before === null || loading) return;
    return run(async () => {
      const read = await page(before, PAGE);
      records = [...records, ...read.records.filter((r) => r.seq < before)];
      nextBefore = read.next_before;
    });
  }

  /**
   * Go to #n: #n and the ten changes above it, the newer ones folded above, focus on #n. `open` also
   * opens its change pane (links); the title bar's Go to # only moves the view (B_HistoryGraphLong).
   * False when #n isn't a change.
   */
  export async function goTo(seq: number, open = false): Promise<boolean> {
    const top = head?.seq ?? Number.MAX_SAFE_INTEGER;
    if (!Number.isInteger(seq) || seq < 1 || seq > top) return false;
    const before = Math.min(seq + WINDOW, top + 1);
    await run(async () => {
      const read = await page(before, before - seq);
      records = read.records;
      head = read.head;
      nextBefore = read.next_before;
      target = (read.records[0]?.seq ?? read.head.seq) < read.head.seq ? seq : null;
      tipsOpen = false;
      onwindow?.(target);
    });
    if (error) return false;
    selected = open ? { kind: 'change', seq } : null;
    await focusRow(`r${seq}`);
    return true;
  }

  /** The folded newer changes, at most 100 a read, until the view is back at the head. */
  function showNewer() {
    const top = records[0]?.seq;
    if (top === undefined || !head || loading) return;
    const count = Math.min(100, head.seq - top);
    return run(async () => {
      const read = await page(top + count + 1, count);
      records = [...read.records.filter((r) => r.seq > top), ...records];
      head = read.head;
      if ((records[0]?.seq ?? 0) >= read.head.seq) leaveWindow();
    });
  }

  /** A live hint: new changes join the top (or the newer fold) and the lanes are read again. */
  let topRead = 0;
  async function refresh() {
    if (!loaded || loading) return;
    const mine = ++topRead;
    try {
      const read = await page(null, PAGE);
      if (mine !== topRead || loading || (head && read.head.seq < head.seq)) return;
      if (target === null) {
        const newest = records[0]?.seq ?? 0;
        const fresh = read.records.filter((r) => r.seq > newest);
        // More new records than one page: the gap can't be stitched, so read again.
        if (fresh.length === read.records.length && read.next_before !== null && records.length) return void load();
        records = [...fresh, ...records];
      }
      head = read.head;
      await readDrafts();
    } catch {
      // The next hint tries again; the graph keeps what it shows.
    }
  }

  onMount(() => {
    // `?tab=graph&seq=38` opens a change, `&draft=<id>` a draft (Drafts' "See it in History").
    const params = new URLSearchParams(location.search);
    const seq = Number(params.get('seq'));
    const draft = params.get('draft');
    void load().then(async () => {
      if (draft) await openDraft(draft);
      else if (seq && records.some((r) => r.seq === seq)) selected = { kind: 'change', seq };
      else if (seq) await goTo(seq, true);
    });
    // A merge hints `schedule`; other draft writes hint `inbox`.
    return live.subscribe(['schedule', 'inbox'], () => void refresh());
  });

  // ── the draft pane / sheet ──
  let detail = $state<DraftDetail | null>(null);
  let detailError = $state('');
  async function openDraft(id: string) {
    selected = { kind: 'draft', id };
    detailError = '';
    if (detail?.id !== id) detail = null;
    try {
      const read = await client.get<DraftDetail>(draftPath(id));
      if (selectedDraft !== id) return;
      detail = read;
      if (read.status === 'open') {
        const preview = await client.get<DraftPreviewView>(draftPath(id, '/preview'));
        previews = { ...previews, [id]: preview };
        conflicts = { ...conflicts, [id]: preview.conflicts.length };
      }
    } catch (e) {
      if (selectedDraft === id) detailError = e instanceof Error ? e.message : 'Could not load the draft.';
    }
  }

  function closePane() {
    const key = selectedKey || (selectedDraft && target !== null && !tipsOpen ? 'tips' : '');
    selected = null;
    detail = null;
    if (key) void focusRow(key);
  }

  async function focusRow(key: string) {
    focusKey = key;
    await tick();
    const row = tableEl?.querySelector<HTMLElement>(`tr[data-row="${CSS.escape(key)}"]`);
    row?.focus({ preventScroll: true });
    row?.scrollIntoView({ block: 'nearest' });
  }

  async function activate(row: Row) {
    focusKey = row.key;
    if (row.kind === 'rec') selected = { kind: 'change', seq: row.record.seq };
    else if (row.kind === 'tip' || row.kind === 'ev') await openDraft(row.draft.id);
    else if (row.kind === 'tips') {
      tipsOpen = true;
      await focusRow(`t-${row.drafts[0]!.id}`);
    } else if (row.kind === 'fold') {
      expanded.add(row.key);
      await focusRow(`r${row.records[0]!.seq}`);
    } else if (row.kind === 'newer') await showNewer();
    else if (row.kind === 'more' && row.canLoad) await older();
  }

  function rowKey(event: KeyboardEvent, row: Row) {
    const tr = event.currentTarget as HTMLElement;
    const buttons = [...tr.querySelectorAll<HTMLElement>('[data-cell-action]')];
    const at = buttons.indexOf(event.target as HTMLElement);
    const index = reachable.findIndex((r) => r.key === row.key);
    const moves: Record<string, number> = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: reachable.length - 1 };
    const move = moves[event.key];
    if (move !== undefined) {
      event.preventDefault();
      const next = reachable[move];
      if (next) void focusRow(next.key);
    } else if (event.key === 'ArrowRight' && at < buttons.length - 1) {
      event.preventDefault();
      buttons[at + 1]!.focus({ preventScroll: true });
    } else if (event.key === 'ArrowLeft' && at >= 0) {
      event.preventDefault();
      (at > 0 ? buttons[at - 1]! : tr).focus({ preventScroll: true });
    } else if ((event.key === 'Enter' || event.key === ' ') && event.target === tr) {
      event.preventDefault();
      void activate(row);
    }
  }

  // Escape closes the draft pane (the change pane closes itself), unless a dialog owns it.
  function windowKey(event: KeyboardEvent) {
    if (wide && selectedDraft && event.key === 'Escape' && !document.querySelector('dialog[open]')) {
      event.preventDefault();
      closePane();
    }
  }

  // ── words ──
  const who = (record: ChangeRecord) => actorName(record.actor, names, known);
  /** "11:48" today (by the server's clock), else "Mon 22:15". */
  function edited(iso: string) {
    const at = shortAt(iso, timezone);
    return week && shortAt(week.generated_at, timezone, false) === shortAt(iso, timezone, false) ? at.slice(-5) : at;
  }
  const when = (iso: string) => (wide ? localAt(iso, timezone) : shortAt(iso, timezone));
  const andNames = (items: string[]) => (items.length < 2 ? (items[0] ?? '') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`);
  const slot = (text: string | null) => (text && /^\d{4}-\d{2}-\d{2}T/.test(text) ? shortAt(text, timezone) : (text ?? '—'));
  // "Drafts at this change": starts and merges here, and the conflicts the opened drafts' previews name here.
  const atChange = $derived.by(() => {
    if (selectedSeq === null) return [];
    const here = draftsAt(selectedSeq, drafts).map(({ draft, what }) => ({
      draft,
      glyph: what === 'starts' ? 'start' : 'merged',
      text: what === 'starts' ? `starts here · ${draft.staged} staged${conflicts[draft.id] ? ` · ${conflicts[draft.id]} conflict${conflicts[draft.id] === 1 ? '' : 's'}` : ''}` : `merged here · from #${draft.base}`,
    }));
    for (const [id, preview] of Object.entries(previews)) {
      const draft = drafts.find((d) => d.id === id);
      const hit = preview.conflicts.find((c) => c.record?.seq === selectedSeq);
      if (draft && hit) here.push({ draft, glyph: 'bad', text: `conflicts with this change · ${hit.field ?? 'the run'} of change ${hit.ops.map((o) => o + 1).join(', ')}` });
    }
    return here;
  });
</script>

<svelte:window onkeydown={windowKey} />

{#snippet mini(kind: string)}
  <svg class="gmini" viewBox="-8 -8 16 16" aria-hidden="true" focusable="false">
    {#if kind === 'change'}<path class="glane__l0" d="M0 -8V8"></path><circle class="glane__rec" r="5"></circle>
    {:else if kind === 'head'}<path class="glane__l0" d="M0 0V8"></path><circle class="glane__head" r="5.5"></circle>
    {:else if kind === 'open'}<path class="glane__d glane__d--open" d="M0 0V8"></path><rect class="glane__tip" x="-5.5" y="-5.5" width="11" height="11" rx="3"></rect>
    {:else if kind === 'bad'}<path class="glane__d glane__d--open" d="M0 0V8"></path><rect class="glane__tip glane__tip--bad" x="-5.5" y="-5.5" width="11" height="11" rx="3"></rect><path class="glane__bang" d="M0 -2.9V0.7M0 2.8v.2"></path>
    {:else if kind === 'merged'}<path class="glane__d glane__d--merged" d="M0 -8V8"></path><circle class="glane__merge" r="6"></circle><circle class="glane__merge-dot" r="2"></circle>
    {:else if kind === 'discarded'}<path class="glane__d glane__d--discarded" d="M0 0V8"></path><path class="glane__cross" d="M-3.8 -3.8L3.8 3.8M3.8 -3.8L-3.8 3.8"></path>
    {:else if kind === 'expired'}<path class="glane__d glane__d--expired" d="M0 0V8"></path><path class="glane__expired" d="M-6.5 0H6.5"></path>
    {:else if kind === 'start'}<path class="glane__d glane__d--open" d="M-4 7V2C-4 -3 4 -2 4 -7"></path>
    {:else if kind === 'start-closed'}<path class="glane__d glane__d--closed" d="M-4 7V2C-4 -3 4 -2 4 -7"></path>
    {/if}
  </svg>
{/snippet}

{#snippet key(extended: boolean)}
  {@const items = extended
    ? [
        ['change', 'History change, newest at the top'],
        ['open', 'Open draft, from the change it started on'],
        ['bad', 'Open draft with conflicts'],
        ['merged', 'Merged: joins the line as a new change'],
        ['discarded', 'Discarded, or expired at the boss reset'],
      ]
    : [
        ['change', 'History change'],
        ...(wide ? [['head', 'Head']] : []),
        ['open', 'Open draft'],
        ['bad', 'Has conflicts'],
        ['merged', 'Merged'],
        ['discarded', 'Discarded'],
        ['expired', 'Expired'],
      ]}
  <ul class="graph__key" class:graph__key--rows={extended} aria-label="Key">
    {#each items as [glyph, text] (glyph)}
      <li>{@render mini(glyph!)}<span>{text}</span></li>
    {/each}
  </ul>
{/snippet}

{#snippet chip(c: Chip)}
  {#if c.kind === 'state'}
    <StatusChip tone={c.tone ?? 'neutral'}>{c.text}</StatusChip>
  {:else}
    {@const closed = c.draft !== undefined && c.draft.status !== 'open'}
    {@const glyph = c.kind === 'merge' ? 'merged' : c.kind === 'discard' ? 'discarded' : c.kind === 'expire' ? 'expired' : closed ? 'start-closed' : 'start'}
    <button
      type="button"
      class="gchip gchip--{c.kind}"
      class:gchip--closed={closed && c.kind !== 'merge'}
      tabindex="-1"
      data-cell-action
      onclick={(event) => {
        event.stopPropagation();
        if (c.draft) void openDraft(c.draft.id);
      }}>{@render mini(glyph)}{c.text}</button
    >
  {/if}
{/snippet}

{#snippet action(text: string, row: Row)}
  <button
    type="button"
    class="btn gx__act"
    tabindex="-1"
    data-cell-action
    disabled={loading}
    onclick={(event) => {
      event.stopPropagation();
      void activate(row);
    }}>{text}</button
  >
{/snippet}

{#snippet chips(row: Row)}
  {#if row.chips.length || row.kind === 'tips' || row.kind === 'fold' || row.kind === 'newer'}
    <div class="gx__chips">
      {#each row.chips as c, i (i)}{@render chip(c)}{/each}
      {#if row.kind === 'tips'}{@render action('Show', row)}
      {:else if row.kind === 'fold'}{@render action(`Show ${row.records.length}`, row)}
      {:else if row.kind === 'newer'}{@render action(`Show ${row.to - row.from + 1}`, row)}{/if}
    </div>
  {/if}
{/snippet}

{#snippet body(row: Row)}
  {#if row.kind === 'rec'}
    {@const r = row.record}
    <span class="gx__l1"
      ><b>{who(r)}</b><span class="chip chip--mono">{SURFACE_LABELS[r.surface] ?? r.surface}</span><span class="gx__time mono">{when(r.at)}</span>{#if row.head}<span class="chip chip--mono gx__headchip">head</span>{/if}</span
    >
    <span class="gx__l2">{describe(r, names, timezone, (id) => titles.get(id)).join(' · ') || `${r.rows.length} row${r.rows.length === 1 ? '' : 's'} changed`}</span>
  {:else if row.kind === 'tip'}
    <span class="gx__l1"><b>{row.draft.title}</b><span class="gx__time mono">{row.draft.author.name} · edited {edited(row.draft.updated_at)}</span></span>
    <span class="gx__l2">{tipLine(row.draft, headSeq)}</span>
  {:else if row.kind === 'tips'}
    <span class="gx__l1"><span><b>{row.drafts.length} open draft{row.drafts.length === 1 ? '' : 's'}</b> · not live</span></span>
    <span class="gx__l2">{row.drafts.map((d) => d.title).join(' · ')}</span>
  {:else if row.kind === 'ev'}
    {@const d = row.draft}
    <span class="gx__l1"
      >{#if d.status === 'expired'}<span><b>{d.title}</b> expired at the boss reset</span>{:else}<span><b>{d.closed_by?.name ?? d.author.name}</b> discarded <b>{d.title}</b></span>{/if}<span class="gx__time mono"
        >{d.closed_at ? when(d.closed_at) : ''}</span
      ></span
    >
    <span class="gx__l2">{d.status === 'expired' ? `Closed by ${d.closed_by?.name ?? 'Kanade'} · ${d.staged} staged · nothing went live` : 'Draft closed · nothing went live'}</span>
  {:else if row.kind === 'fold'}
    <span class="gx__l1"><span><b class="mono">#{row.records.at(-1)!.seq} – #{row.records[0]!.seq}</b> · {row.records.length} changes</span></span>
    <span class="gx__l2">No draft starts or ends in these{#if row.lanes.length}; {andNames(row.lanes.map((l) => l.draft.title))} run{row.lanes.length === 1 ? 's' : ''} through{/if}</span>
  {:else if row.kind === 'newer'}
    <span class="gx__l1"><span><b class="mono">#{row.from} – #{row.to}</b> · {row.to - row.from + 1} newer change{row.to === row.from ? '' : 's'}, head <span class="mono">#{row.to}</span></span></span>
    <span class="gx__l2"
      >{#if row.starting.length}{andNames(row.starting.map((d) => d.title))} start{row.starting.length === 1 ? 's' : ''} in these{:else}No draft starts in these{/if}{#if row.inside.length}; {row.inside.length} closed draft{row.inside.length === 1
          ? ''
          : 's'} begin and end inside ({andNames(row.inside.map((d) => `${d.title}, ${closeWord(d)}`))}){/if}</span
    >
  {:else if row.kind === 'more'}
    <span class="gx__l1"
      >{#if row.canLoad}<button
          type="button"
          class="btn gx__act"
          tabindex="-1"
          data-cell-action
          disabled={loading}
          onclick={(event) => {
            event.stopPropagation();
            void older();
          }}>Older changes</button
        >{:else}<span>The start of History</span>{/if}</span
    >
    {#if row.older}<span class="gx__l2">#{row.older} and older · {row.older} change{row.older === 1 ? '' : 's'}</span>{/if}
  {/if}
{/snippet}

{#snippet identity(row: Row)}
  {#if row.kind === 'rec'}#{row.record.seq}
  {:else if row.kind === 'tip' || row.kind === 'tips'}<Icon name="layers" /><span class="vh">{row.kind === 'tip' ? 'Open draft' : 'Open drafts'}</span>
  {:else if row.kind === 'ev'}<span aria-hidden="true">–</span><span class="vh">Draft closed</span>
  {:else if row.kind === 'fold' || row.kind === 'newer'}<Icon name="more-vertical" /><span class="vh">Folded changes</span>
  {:else}<span class="vh">More</span>{/if}
{/snippet}

{#snippet draftFacts(d: DraftDetail)}
  {@const started = records.find((r) => r.seq === d.base)}
  {@const since = records.filter((r) => r.seq > d.base).slice(0, 5).reverse()}
  {@const preview = previews[d.id]}
  <dl class="graph-facts">
    <dt class="cap">Started on</dt>
    <dd
      ><button type="button" class="linklike mono" onclick={() => (started ? (selected = { kind: 'change', seq: d.base }) : void goTo(d.base, true))}>#{d.base}</button>{#if started}
        · {who(started)} · {localAt(started.at, timezone)}{/if}</dd
    >
    {#if d.status === 'open'}
      <dt class="cap">Head</dt>
      <dd>
        <span class="mono">#{headSeq}</span>{#if headSeq > d.base}
          · {headSeq - d.base} change{headSeq - d.base === 1 ? '' : 's'} since{#if since.length}: {#each since as r, i (r.seq)}{i ? ', ' : ''}<span class="mono">#{r.seq}</span> {who(r)}{/each}{/if}{:else}
          · up to date{/if}
      </dd>
    {:else}
      <dt class="cap">{d.status === 'merged' ? 'Merged as' : 'Closed'}</dt>
      <dd>{#if d.merged_seq !== null}<span class="mono">#{d.merged_seq}</span> · {/if}{closeWord(d)}{d.closed_by ? ` by ${d.closed_by.name}` : ''} · {localAt(d.updated_at, timezone)}</dd>
    {/if}
  </dl>
  {#if preview?.conflicts.length}
    <h3 class="cap graph-facts__head">Conflicts · {preview.conflicts.length}</h3>
    {#each preview.conflicts as c (c.key)}
      {@const run = c.run_id ? week?.runs.find((r) => r.id === c.run_id) : undefined}
      <div class="draftcfl graph-cfl" data-fid="drafts-conflict">
        <div class="draftcfl__head">
          <b>{run ? runTitle(run) : (c.run_id ?? c.fixed_id ?? 'A change')}{c.kind === 'both_changed' && c.field ? ` · ${c.field}` : ''}</b>
          <StatusChip tone="risk">{c.kind === 'both_changed' ? 'Both changed' : c.kind === 'upstream_removed' ? removalLabel(c) : 'Can’t apply'}</StatusChip>
        </div>
        <p class="draftcfl__text">
          {#if c.kind === 'both_changed'}Yours <span class="mono">{slot(c.yours)}</span> · now <span class="mono">{slot(c.now)}</span>{#if c.record}&#32;(<span class="mono">#{c.record.seq}</span> {c.record.actor.name}){/if}{:else}{c.record ? `${c.record.actor.name} changed it.` : c.message} Change{c.ops.length === 1 ? '' : 's'} {c.ops.map((o) => o + 1).join(' and ')} can’t apply.{/if}
        </p>
      </div>
    {/each}
  {:else if d.status === 'open' && !preview}
    <p class="note">Counting conflicts…</p>
  {/if}
{/snippet}

{#snippet draftMeta(d: DraftDetail)}
  <p class="history-detail__meta">{d.author.name} · edited {edited(d.updated_at)} · {d.staged} staged · {d.status === 'merged' ? 'live' : 'not live'}</p>
{/snippet}

{#snippet openLink(d: DraftDetail)}
  <a class="btn btn--primary btn--key graph-open" data-fid="graph-open-draft" href={draftHref(d)}><Icon name="layers" />Open draft</a>
{/snippet}

{#snippet paneDrafts()}
  {#if atChange.length}
    <section class="graph-at" data-fid="graph-pane-drafts" aria-labelledby="graph-at-h">
      <h3 class="cap" id="graph-at-h">Drafts at this change · {atChange.length}</h3>
      <ul class="graph-at__rows">
        {#each atChange as item, i (i)}
          <li class="graph-at__row">
            {@render mini(item.glyph)}
            <span class="graph-at__lines"><b>{item.draft.title}</b><span>{item.text}</span></span>
            <button type="button" class="btn" aria-label="Open draft {item.draft.title}" onclick={() => void openDraft(item.draft.id)}>Open</button>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{/snippet}

{#if error && !loaded}
  <div class="graph graph--state"><LoadError thing="the graph" reason={error} onretry={() => void load()} level={3} /></div>
{:else if !loaded}
  <div class="graph graph--state"><LoadingState text="Loading the graph…" /></div>
{:else if empty}
  <div class="inbox-empty graph-empty" data-fid="empty-body">
    {#snippet ways()}
      <a class="btn btn--primary" href="/drafts"><Icon name="plus" />New draft</a>
      <button class="btn" type="button" onclick={ontimeline}>Open Timeline</button>
    {/snippet}
    <StateNote icon="git-branch" title="No drafts to draw yet" actions={ways}
      >A draft branches off the change it was started on and joins the line when you merge it. Your {headSeq} change{headSeq === 1 ? ' is' : 's are'} in the Timeline.</StateNote
    >
    <section class="graph-empty__key" data-fid="graph-legend" aria-labelledby="graph-key-h">
      <h3 class="cap" id="graph-key-h">How the graph reads</h3>
      {@render key(true)}
    </section>
  </div>
{:else}
  <div class="graph" class:graph--narrow={!wide} data-fid="graph">
    {#if target !== null && records.length}
      <p class="graph__window"><b>Showing #{records.at(-1)!.seq} – #{records[0]!.seq}</b> <span>of {headSeq} · after “Go to #{target}”.{wide ? ' Quiet stretches fold; lanes run through folds.' : ''}</span></p>
    {/if}
    <div class="graph__legend" data-fid="graph-legend">
      {#if view === 'list'}<span class="graph__note">{wide ? 'The same rows without the lanes: this table is what a screen reader reads in Graph view too.' : 'The same rows without the lanes.'}</span>{:else}{@render key(false)}{/if}
      {#if wide}<span class="graph__keys" data-fid="graph-keys"><kbd class="kbd">↑</kbd><kbd class="kbd">↓</kbd> rows · <kbd class="kbd">→</kbd> drafts · <kbd class="kbd">Enter</kbd> opens</span>{/if}
    </div>
    {#if error}<p class="flash flash--error" role="alert">{error}</p>{/if}
    <div class="graph__scroll" data-fid="graph-scroll">
      <table class="gx gx--lanes-{Math.min(lanes, 8)}" class:gx--narrow={!wide} role="grid" aria-label={label} aria-busy={loading} data-fid="graph-table" bind:this={tableEl}>
        <caption class="vh">{CAPTION}</caption>
        <thead>
          <tr>
            {#if view === 'graph'}<th scope="col" class="gx__lanecol" aria-hidden="true"></th>{/if}
            <th scope="col" class="gx__numcol">#</th>
            <th scope="col">{wide ? 'Change' : 'Change · drafts'}</th>
            {#if wide}<th scope="col" class="gx__draftcol" class:gx__draftcol--wide={target !== null}>Drafts</th>{/if}
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.key)}
            {#if row.kind === 'sec'}
              <tr class="gx__sec">
                {#if view === 'graph'}<GraphLanes lanes={row.lanes} line={row.line} node={row.node} {width} {x} {selectedDraft} bad={(id) => Boolean(conflicts[id])} />{/if}
                <td colspan={wide ? 3 : 2}><h3 class="gx__sechead">{row.label}</h3></td>
              </tr>
            {:else}
              {@const isSel = row.key === selectedKey}
              <tr
                class="gx__row gx__row--{row.kind}"
                class:is-sel={isSel}
                data-fid="graph-row"
                data-row={row.key}
                tabindex={row.key === stop ? 0 : -1}
                aria-selected={isSel}
                onclick={() => void activate(row)}
                onkeydown={(event) => rowKey(event, row)}
                onfocus={(event) => {
                  if (event.target === event.currentTarget) focusKey = row.key;
                }}
              >
                {#if view === 'graph'}<GraphLanes lanes={row.lanes} line={row.line} node={row.node} {width} {x} selected={isSel} {selectedDraft} bad={(id) => Boolean(conflicts[id])} />{/if}
                <th scope="row" class="gx__id mono">{@render identity(row)}</th>
                <td class="gx__change">{@render body(row)}{#if !wide}{@render chips(row)}{/if}</td>
                {#if wide}<td class="gx__drafts">{@render chips(row)}</td>{/if}
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{/if}

{#if selectedRecord}
  <HistoryDetail {wide} extra={paneDrafts} record={selectedRecord} week={selectedRecord.weeks[0] ?? ''} {timezone} {names} runTitle={(id) => titles.get(id)} onclose={closePane} {onrevert} {onrestore} {onraw} />
{:else if selectedDraft && wide}
  <aside class="side-pane side-pane--history graph-pane" data-fid="graph-draft-pane" aria-label="Draft {detail?.title ?? ''}">
    {#if detail}
      <div class="history-detail__head" data-fid="history-pane-head">
        <div class="history-detail__top">
          <p class="cap">Draft · {detail.status} · {detail.scope === 'weekly' ? 'weekly timings' : detail.week ? `boss week of ${shortDate(detail.week)}` : 'nothing staged'}</p>
          <button class="btn btn--ghost history-detail__close" type="button" aria-label="Close draft details" onclick={closePane}>×</button>
        </div>
        <h2>{detail.title}</h2>
        {@render draftMeta(detail)}
      </div>
      <div class="history-detail__body graph-pane__body">{@render draftFacts(detail)}</div>
      <div class="graph-pane__foot">
        <DecisionCard label="Open in Drafts" overline="">
          <p class="decision__why graph-pane__note" data-fid="decision-note">
            {detail.status !== 'open' ? 'Closed drafts are read-only; Drafts keeps them.' : conflicts[detail.id] ? 'Settle the conflicts and rebase in the draft. The graph only shows it.' : 'Merge or keep editing in the draft. The graph only shows it.'}
          </p>
          {@render openLink(detail)}
        </DecisionCard>
      </div>
    {:else if detailError}
      <LoadError thing="the draft" reason={detailError} onretry={() => void openDraft(selectedDraft)} level={3} />
    {:else}
      <LoadingState text="Loading the draft…" />
    {/if}
  </aside>
{:else if selectedDraft}
  <Modal open title={detail?.title ?? 'Draft'} eyebrow={detail ? `Draft · ${detail.status}${detail.week ? ` · boss week of ${shortDate(detail.week)}` : ''}` : 'Draft'} narrow className="graphsheet" fid="graph-sheet" fidParts={{}} onclose={closePane}>
    {#if detail}
      {@render draftMeta(detail)}
      {@render draftFacts(detail)}
      {@render openLink(detail)}
    {:else if detailError}
      <LoadError thing="the draft" reason={detailError} onretry={() => void openDraft(selectedDraft)} level={3} />
    {:else}
      <LoadingState text="Loading the draft…" />
    {/if}
  </Modal>
{/if}
