<!--
  One guide (boards G_List … G_RunStates, G_Phone*): DraftView's two
  columns. The editor: head (portrait, "Guide · #channel", title with
  rename, who saved which version), the unconfirmed-post card when a post
  could not be confirmed, "Saving never posts", the channel (one guide per
  channel), the blocks and Config's save bar with Save draft. The pane: the
  preview of what is on screen (unsaved edits included) and the Publish
  decision (off while there are unsaved changes), or a run's progress with
  Cancel, or how it ended. A save refused as stale never merges: copy your
  text blocks or load the newer version. Phones: Edit / Preview tabs and
  Drafts' bottom bar.
-->
<script lang="ts">
  import type { BossRow, Channel, GuideBlock, GuideBlocksRefused, GuideDetail, GuidePlan, GuidePreview as Preview, GuidePublishResult, GuideRow, GuideRun, GuideSaved, Reminders } from '@kanade/api-types';
  import { ApiRequestError, createClient, newIdempotencyKey } from '@kanade/client';
  import { Icon, Modal, PendingLabel, Select, StatusChip, Toaster, WavyProgress } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import SaveBar from '../config/SaveBar.svelte';
  import { editedAt } from '../drafts/drafts';
  import { actorName } from '../history/describe';
  import { directory } from '../names/directory.svelte';
  import { Resource, errorText, send } from '../resource.svelte';
  import { discordLink } from '../shared/discordLink.svelte';
  import { channelOptions } from './channels';
  import GuideBlocks from './GuideBlocks.svelte';
  import GuidePreview from './GuidePreview.svelte';
  import { blockChanges, guidePath, OP_WORD, opLabel, plural, positionsByBlock, runSummary, stopWords, units, CONTENT_LIMIT, NOTE_LIMIT } from './guides';
  import { follow } from './outcome.svelte';
  import PublishPlan from './PublishPlan.svelte';

  let {
    id,
    row,
    rows,
    channels,
    timeZone,
    now,
    bar,
    toaster,
    onchanged,
    ondeleted,
  }: {
    id: string;
    /** The list's row: chip, reason, messages, live progress. */
    row: GuideRow;
    rows: GuideRow[];
    channels: Channel[];
    timeZone: string;
    /** The server's clock (`Week.generated_at`). */
    now: string;
    /** One pane (below 900 px): Edit / Preview tabs and the bottom bar. */
    bar: boolean;
    toaster: Toaster;
    onchanged: () => void;
    ondeleted: (message: string) => void;
  } = $props();
  const uid = $props.id();

  // svelte-ignore state_referenced_locally
  const detail = new Resource<GuideDetail>(guidePath(id));
  const bossList = new Resource<BossRow[]>('/api/admin/bosses');
  $effect(() => void detail.load());
  $effect(() => void bossList.load());

  const names = (who: string) => directory.label('member', who);
  const who = (actor: { kind: GuideRow['updated_by']['kind']; id: string }) => actorName(actor, names, (m) => directory.members.has(m));
  const channelName = (channel: string) => channels.find((c) => c.id === channel)?.name ?? (channel === row.channel_id ? (row.channel_name ?? '#unknown-channel') : '#unknown-channel');
  const at = (iso: string) => editedAt(iso, timeZone, now);

  // ── the draft: what is on screen, against the version it started from ──
  let base = $state<GuideDetail | null>(null);
  let title = $state('');
  let channel = $state('');
  let blocks = $state<GuideBlock[]>([]);
  let openId = $state<string | null>(null);

  function adopt(from: GuideDetail) {
    base = from;
    title = from.guide.title;
    channel = from.guide.channel_id ?? '';
    blocks = $state.snapshot(from.blocks) as GuideBlock[];
    if (openId && !blocks.some((b) => b.id === openId)) openId = null;
  }

  const dirty = $derived(Boolean(base) && (title !== base!.guide.title || channel !== (base!.guide.channel_id ?? '') || JSON.stringify(blocks) !== JSON.stringify(base!.blocks)));
  // A newer read replaces the draft only while nothing is unsaved (a stale save says so instead).
  $effect(() => {
    const read = detail.data;
    if (!read) return;
    untrack(() => {
      if (!base || !dirty) adopt(read);
      else base = { ...read, blocks: base.blocks, guide: { ...read.guide, version: base.guide.version, title: base.guide.title, channel_id: base.guide.channel_id } };
    });
  });

  const changes = $derived(
    base
      ? blockChanges(base.blocks, blocks, {
          channel: [base.guide.channel_id ? channelName(base.guide.channel_id) : '—', channel ? channelName(channel) : '—'],
          title: [base.guide.title, title],
        })
      : [],
  );
  const over = $derived(blocks.some((b) => (b.kind === 'text' && units(b.content) > CONTENT_LIMIT) || (b.kind === 'bosses' && b.entries.some((e) => units(e.note ?? '') > NOTE_LIMIT))));
  const savedIds = $derived(new Set((base?.blocks ?? []).map((b) => b.id)));

  /** For the page's leave guard: each unsaved change in words. */
  export function unsaved(): string[] {
    return dirty ? changes.map((c) => `${c.label} (${c.to})`) : [];
  }

  // ── the preview: the blocks on screen, rendered by the server ──
  let preview = $state<Preview | null>(null);
  let problems = $state<Record<string, string[]>>({});
  let previewError = $state('');
  const client = createClient();
  $effect(() => {
    const body = JSON.stringify(blocks);
    const timer = setTimeout(() => void render(body), 300);
    return () => clearTimeout(timer);
  });
  async function render(body: string) {
    try {
      const drawn = await client.post<Preview>(guidePath(id, '/render'), { blocks: JSON.parse(body) }, { ambient: true });
      if (body !== JSON.stringify(blocks)) return;
      preview = drawn;
      problems = {};
      previewError = '';
    } catch (error) {
      if (!(error instanceof ApiRequestError)) return;
      const refused = error.body as unknown as GuideBlocksRefused | null;
      if (error.status === 422 && refused?.problems) problems = byBlock(refused);
      else previewError = errorText(error);
    }
  }
  function byBlock(refused: GuideBlocksRefused): Record<string, string[]> {
    const out: Record<string, string[]> = {};
    for (const p of refused.problems) (out[p.block_id ?? ''] ??= []).push(p.message);
    return out;
  }
  const positions = $derived(positionsByBlock(preview?.slots ?? []));
  const here = $derived(openId ? (positions.get(openId)?.[0] ?? null) : null);
  const target = $derived(channel ? channelName(channel) : 'no channel yet');

  // ── saving ──
  let saving = $state(false);
  let saveError = $state('');
  let stale = $state<GuideDetail | null>(null);
  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!base || !dirty || over || saving) return;
    saving = true;
    saveError = '';
    try {
      const saved = await client.patch<GuideSaved>(guidePath(id), { version: base.guide.version, title: title.trim(), channel_id: channel || null, blocks });
      await detail.load();
      if (detail.data) adopt(detail.data);
      toaster.show({ message: `${title.trim()} saved as version ${saved.version}. Nothing was posted.`, tone: 'ok' });
      onchanged();
    } catch (error) {
      if (!(error instanceof ApiRequestError)) saveError = 'Something went wrong.';
      else if (error.status === 409 && error.body?.error === 'stale') {
        const read = await send((c) => c.get<GuideDetail>(guidePath(id)));
        if (read.ok) stale = read.value;
        else saveError = errorText(error);
      } else if (error.status === 422 && (error.body as unknown as GuideBlocksRefused | null)?.problems) {
        problems = byBlock(error.body as unknown as GuideBlocksRefused);
        saveError = 'Fix the blocks marked below, then save again.';
      } else saveError = errorText(error);
    } finally {
      saving = false;
    }
  }
  function discard() {
    if (base) adopt(base);
    saveError = '';
  }
  // Copy my block n text (a stale save): each changed text block, never merged.
  const staleCopies = $derived(
    stale && base
      ? blocks.flatMap((b, i) => (b.kind === 'text' && JSON.stringify(base!.blocks.find((s) => s.id === b.id)) !== JSON.stringify(b) ? [{ n: i + 1, text: b.content }] : []))
      : [],
  );
  async function copyText(text: string, n: number) {
    try {
      await navigator.clipboard.writeText(text);
      toaster.show({ message: `Copied block ${n}’s text.`, tone: 'ok' });
    } catch {
      toaster.show({ message: 'Couldn’t copy; select the text and copy it by hand.', tone: 'error' });
    }
  }
  function loadNewer() {
    if (!stale) return;
    adopt(stale);
    stale = null;
    onchanged();
  }

  // ── runs: the newest one, followed while live ──
  let run = $state<GuideRun | null>(null);
  let watched = $state(false);
  let outcomeShown = $state(false);
  const lastRun = $derived(detail.data?.guide.last_run?.id ?? null);
  const live = $derived(run !== null && (run.state === 'queued' || run.state === 'running'));
  async function readRun(runId: string) {
    const read = await send((c) => c.get<GuideRun>(guidePath(id, `/publishes/${encodeURIComponent(runId)}`)));
    if (!read.ok) return;
    const was = run;
    run = read.value;
    const ended = was && (was.state === 'queued' || was.state === 'running') && run.state !== 'queued' && run.state !== 'running';
    if (ended) {
      await detail.load();
      onchanged();
    }
  }
  $effect(() => {
    if (lastRun) void readRun(lastRun);
  });
  $effect(() => {
    if (!live || !run) return;
    const runId = run.id;
    watched = true;
    outcomeShown = true;
    const timer = setInterval(() => void readRun(runId), 700);
    return () => clearInterval(timer);
  });
  function started(next: GuideRun) {
    run = next;
    watched = true;
    outcomeShown = true;
    follow({ guide: id, run: next.id, title: title || row.title, channel: channelName(next.channel_id) });
    void detail.load();
    onchanged();
  }
  let cancelling = $state(false);
  let cancelKey = '';
  async function cancel() {
    if (!run || cancelling) return;
    cancelling = true;
    cancelKey ||= newIdempotencyKey();
    const r = run;
    const done = await send((c) => c.post<GuideRun>(guidePath(id, `/publishes/${encodeURIComponent(r.id)}/cancel`), {}, { idempotencyKey: cancelKey }));
    cancelling = false;
    if (done.ok) run = done.value;
    else toaster.show({ message: `Couldn’t cancel: ${done.message}`, tone: 'error' });
  }

  // ── an unconfirmed post (Search again / Mark as not posted) ──
  const unknown = $derived(detail.data?.slots.find((s) => s.state === 'unknown') ?? null);
  const unknownBlock = $derived(unknown ? (preview?.slots.find((s) => s.position === unknown.position)?.block_id ?? null) : null);
  const searching = $derived(live && run?.kind === 'search');
  let found = $state(false);
  let searchKey = '';
  let markOpen = $state(false);
  let markBusy = $state(false);
  let markError = $state('');
  async function search() {
    searchKey ||= newIdempotencyKey();
    const done = await send((c) => c.post<GuidePublishResult>(guidePath(id, '/search'), {}, { idempotencyKey: searchKey }));
    searchKey = '';
    if (!done.ok) {
      if (done.status === 503) offline = done.message;
      toaster.show({ message: done.message, tone: 'error' });
      return;
    }
    if (done.value.publish) started(done.value.publish);
  }
  $effect(() => {
    // A search that ended with nothing left unknown found the post.
    if (run?.kind === 'search' && run.state === 'done' && watched && detail.data && !unknown) found = true;
  });
  let markKey = '';
  async function markNotPosted() {
    if (!unknown || markBusy) return;
    markBusy = true;
    markKey ||= newIdempotencyKey();
    const slot = unknown;
    const done = await send((c) => c.post<{ message: string }>(guidePath(id, '/slots/not-posted'), { channel_id: slot.channel_id, position: slot.position, confirm: true }, { idempotencyKey: markKey }));
    markBusy = false;
    if (!done.ok) {
      markError = done.message;
      return;
    }
    markKey = '';
    markOpen = false;
    toaster.show({ message: done.value.message, tone: 'ok' });
    await detail.load();
    onchanged();
  }

  // ── publishing ──
  let planOpen = $state(false);
  let offline = $state('');
  const published = $derived((detail.data?.slots ?? []).filter((s) => s.state !== 'gone').length);
  const blocked = $derived.by(() => {
    if (offline) return { lead: 'Can’t publish now:', why: 'Discord is offline. Publishing needs the bot connected; the saved version waits.' };
    if (live) return { lead: 'Publishing:', why: 'wait for it to finish or cancel it.' };
    if (unknown) return { lead: 'Blocked:', why: `settle message ${unknown.position + 1} first.` };
    if (dirty) return { lead: 'Can’t publish yet:', why: `save your ${plural(changes.length, 'change')} first.` };
    if (!base?.guide.channel_id) return { lead: 'Can’t publish yet:', why: 'choose a channel and save first.' };
    if (!base.blocks.length) return { lead: 'Can’t publish yet:', why: 'add a block first.' };
    if (row.status === 'published') return { lead: 'Up to date:', why: `nothing changed since ${row.last_run?.finished_at ? at(row.last_run.finished_at) : 'the last publish'}.` };
    return null;
  });
  const consequence = $derived.by(() => {
    const v = base?.guide.version ?? 0;
    if (dirty) return 'Publish uses the saved draft. You review the plan before anything posts.';
    if (row.status === 'published') return `Discord shows version ${v}, the saved draft. Nothing to publish.`;
    if (unknown) return `Version ${v} · messages from ${unknown.position + 1} on are not confirmed yet.`;
    return `Version ${v} · you review the plan before anything posts.`;
  });
  const checkedAt = () => at(now || new Date().toISOString());

  // ── delete: unpublish first, then delete the guide (approved default 7) ──
  let deleteOpen = $state(false);
  let unpublishPlan = $state<GuidePlan | null>(null);
  let deleteBusy = $state(false);
  let deleteError = $state('');
  let deleteKey = '';
  async function askDelete() {
    deleteError = '';
    unpublishPlan = null;
    deleteKey = newIdempotencyKey();
    deleteOpen = true;
    if (published) {
      const read = await send((c) => c.get<GuidePlan>(guidePath(id, '/plan?kind=unpublish')));
      if (read.ok) unpublishPlan = read.value;
      else deleteError = read.message;
    }
  }
  async function confirmDelete() {
    if (deleteBusy || !base) return;
    deleteBusy = true;
    deleteError = '';
    if (published) {
      const p = unpublishPlan;
      if (!p) return void (deleteBusy = false);
      const done = await send((c) => c.post<GuidePublishResult>(guidePath(id, '/unpublish'), { plan_hash: p.plan_hash, confirm: true }, { idempotencyKey: deleteKey }));
      deleteBusy = false;
      if (!done.ok) {
        if (done.status === 503) offline = done.message;
        deleteError = done.message;
        return;
      }
      deleteOpen = false;
      if (done.value.publish) started(done.value.publish);
      return;
    }
    const v = base.guide.version;
    const done = await send((c) => c.delete<{ message: string }>(guidePath(id), { idempotencyKey: deleteKey }, { version: v }));
    deleteBusy = false;
    if (!done.ok) {
      deleteError = done.message;
      return;
    }
    deleteOpen = false;
    ondeleted(done.value.message);
  }

  // "Open in Discord": the guild comes from a posted card's link (the guide API names no guild).
  let guild = $state<string | null>(null);
  $effect(() => {
    if (!run || run.state !== 'done' || guild !== null) return;
    void send((c) => c.get<Reminders>('/api/admin/reminders')).then((read) => {
      const url = read.ok ? [...read.value.sent, ...read.value.upcoming].find((r) => r.url)?.url : undefined;
      guild = /\/channels\/(\d+)\//.exec(url ?? '')?.[1] ?? '';
    });
  });
  const firstMessage = $derived((detail.data?.slots ?? []).find((s) => s.state === 'posted' && s.channel_id === run?.channel_id && s.position === 0)?.message_id ?? null);
  const discordUrl = $derived(guild && run && firstMessage ? `https://discord.com/channels/${guild}/${run.channel_id}/${firstMessage}` : null);

  // Phones: Edit / Preview.
  let tab = $state<'edit' | 'preview'>('edit');
  const meta = $derived.by(() => {
    const g = detail.data?.guide;
    if (!g || !base) return '';
    const last = g.last_run?.state === 'done' && g.last_run.kind === 'publish' && g.last_run.finished_at ? `published ${at(g.last_run.finished_at)}` : g.messages ? 'published' : 'not published yet';
    return bar ? `version ${base.guide.version} · ${last}` : `${who(g.updated_by)} · saved ${at(g.updated_at)} · version ${base.guide.version} · ${last}`;
  });
  const portrait = $derived(row.portrait);
  const editForm = `${uid}-form`;
  const options = $derived(channelOptions(channels, rows, id));
  const wasChannel = $derived(base?.guide.channel_id && channel && channel !== base.guide.channel_id ? channelName(base.guide.channel_id) : null);
  const runTotal = $derived(run?.progress.total ?? run?.ops.length ?? 0);

  let renaming = $state(false);
  let renameButton = $state<HTMLButtonElement>();
  async function startRename() {
    renaming = true;
    await tick();
    document.getElementById(`${uid}-title`)?.focus();
  }
  async function stopRename() {
    renaming = false;
    await tick();
    renameButton?.focus({ preventScroll: true });
  }
</script>

{#snippet runOps(r: GuideRun)}
  <ol class="draftops gplan__ops" aria-label="Changes and their outcome">
    {#each r.ops as op (op.ordinal)}
      {@const kind = op.kind === 'create' ? 'add' : op.kind === 'forget' ? 'keep' : op.kind}
      {@const slot = preview?.slots.find((s) => s.position === op.position)}
      <li class="draftop gplan__op gplan__op--{kind}" class:gplan__op--wait={op.outcome === 'pending' && r.state !== 'running'} data-fid="guide-progress-op">
        <span class="draftop__ord mono">{op.position + 1}</span>
        <span class="draftop__kind">{OP_WORD[op.kind]}</span>
        <div class="draftop__text">
          <span class="draftop__what"
            ><b>{op.kind !== 'delete' && slot ? opLabel({ kind: op.kind, position: op.position, channel_id: op.channel_id, message_id: null, block_kind: (blocks.find((b) => b.id === slot.block_id)?.kind ?? null) as GuideBlock['kind'] | null, summary: slot.content.split('\n')[0] || slot.embeds[0]?.title || null }) : `Message ${op.position + 1}`}</b></span
          >
        </div>
        <span class="gplan__state">
          {#if op.outcome === 'done' || op.outcome === 'recovered' || op.outcome === 'gone'}<StatusChip tone="ok"><Icon name="check" />{op.outcome === 'recovered' ? 'found' : op.outcome === 'gone' ? 'already gone' : 'done'}</StatusChip>
          {:else if op.outcome === 'pending'}{#if r.state === 'running'}<StatusChip tone="neutral">{op.kind === 'create' ? 'posting…' : op.kind === 'edit' ? 'editing…' : 'deleting…'}</StatusChip>{:else}<span class="gplan__waiting">not sent</span>{/if}
          {:else if op.outcome === 'skipped'}<StatusChip tone="neutral">not sent</StatusChip>
          {:else}<StatusChip tone="risk">{op.outcome === 'unknown' ? 'unconfirmed' : 'refused'}</StatusChip>{/if}
        </span>
      </li>
    {/each}
    {#each Array.from({ length: Math.max(0, runTotal - r.ops.length) }) as _, k (k)}
      <li class="draftop gplan__op gplan__op--wait" data-fid="guide-progress-op">
        <span class="draftop__ord mono">–</span>
        <span class="draftop__kind">Next</span>
        <div class="draftop__text"><span class="draftop__what"><b>Change {r.ops.length + k + 1} of {runTotal}</b></span></div>
        <span class="gplan__state"><span class="gplan__waiting">waiting</span></span>
      </li>
    {/each}
  </ol>
{/snippet}

{#snippet publishKey()}
  <button
    class="btn btn--primary btn--key drafts__merge"
    data-fid="guide-publish"
    type="button"
    aria-disabled={blocked !== null}
    aria-describedby={blocked ? `${uid}-why` : undefined}
    onclick={() => {
      if (!blocked) planOpen = true;
    }}><Icon name="message-square" />{bar ? 'Publish…' : 'Review and publish…'}</button
  >
{/snippet}

{#snippet why()}
  {#if blocked}<p class="decision__why" data-fid="decision-note" id="{uid}-why"><strong>{blocked.lead}</strong> {blocked.why}{#if offline} <button class="btn btn--ghost" type="button" onclick={() => (offline = '')}>Try again</button>{/if}</p>{/if}
{/snippet}

{#snippet decision()}
  {#if live && run}
    <aside class="decision-card" aria-label="Cancel publishing" data-fid="decision">
      <p class="cap" aria-hidden="true">Publishing</p>
      <button class="btn btn--danger decision__reject guide-cancel" data-fid="guide-cancel" type="button" disabled={cancelling || Boolean(run.cancel_requested_at)} onclick={() => void cancel()}
        ><Icon name="x" /><PendingLabel pending={cancelling} label="Cancelling…">{run.cancel_requested_at ? 'Cancelling…' : run.kind === 'search' ? 'Cancel search' : 'Cancel publishing'}</PendingLabel></button
      >
      <p class="decision__why" data-fid="decision-note"><strong>Cancel:</strong> it stops after the message in flight. Messages already posted stay recorded, so the next publish carries on from there.</p>
    </aside>
  {:else if outcomeShown && run && run.state === 'done' && run.kind !== 'search'}
    <aside class="decision-card" aria-label="Published" data-fid="decision">
      <p class="cap" aria-hidden="true">{run.kind === 'unpublish' ? 'Unpublished' : 'Published'}</p>
      <p class="proposal__consequence" data-fid="decision-consequence">{run.kind === 'unpublish' ? 'Nothing of this guide is in Discord now. History has the request and the outcome.' : `Discord shows version ${run.version}. History has the request and the outcome.`}</p>
      {#if run.kind === 'unpublish'}
        <button class="btn btn--risk btn--key drafts__merge" type="button" data-fid="guide-delete" onclick={() => void askDelete()}><Icon name="trash-2" />Delete guide…</button>
      {:else if discordUrl}
        <a class="btn btn--primary btn--key drafts__merge" data-fid="guide-open-discord" {...discordLink(discordUrl)}><Icon name="external-link" />Open in Discord</a>
      {/if}
      <span class="decision__spacer" aria-hidden="true"></span>
      <div class="decision__footrow drafts__foot" data-fid="decision-foot">
        <button class="btn btn--ghost" type="button" onclick={() => (outcomeShown = false)}>Back to the preview</button>
      </div>
    </aside>
  {:else}
    <aside class="decision-card" aria-label="Publish this guide" data-fid="decision">
      <p class="cap" aria-hidden="true">Publish</p>
      <p class="proposal__consequence" data-fid="decision-consequence">{consequence}</p>
      {@render publishKey()}
      {@render why()}
      <span class="decision__spacer" aria-hidden="true"></span>
      <div class="decision__footrow drafts__foot" data-fid="decision-foot">
        <span class="decision__foot">Saving never posts.</span>
        <button class="btn btn--danger decision__reject" data-fid="guide-delete" type="button" disabled={live} onclick={() => void askDelete()}>Delete guide…</button>
      </div>
    </aside>
  {/if}
{/snippet}

{#snippet phoneBar()}
  <aside class="decision-card" aria-label="Save and publish" data-fid="decision">
    {#if live && run}
      <p class="decision__why" data-fid="decision-note"><strong>{run.kind === 'search' ? 'Searching' : 'Publishing'} {run.progress.done} of {runTotal}.</strong> It carries on if you leave.</p>
      <div class="drafts__barrow">
        <button class="btn btn--danger btn--key drafts__merge" data-fid="guide-cancel" type="button" disabled={cancelling || Boolean(run.cancel_requested_at)} onclick={() => void cancel()}><Icon name="x" />Cancel publishing</button>
      </div>
    {:else}
      <p class="decision__why" data-fid="decision-note" id="{uid}-why">
        {#if dirty}<strong>{plural(changes.length, 'unsaved change')}.</strong> Publish uses the saved draft.
        {:else if blocked}<strong>{blocked.lead}</strong> {blocked.why}
        {:else}<strong>Version {base?.guide.version} saved.</strong> You review the plan before anything posts.{/if}
      </p>
      <div class="drafts__barrow">
        {#if dirty}
          <button class="btn drafts__discard" type="button" aria-disabled="true" data-fid="guide-publish">Publish…</button>
          <button class="btn btn--primary btn--key drafts__merge" type="submit" form={editForm} data-fid="guide-save" disabled={over || saving}><Icon name="check" /><PendingLabel pending={saving} label="Saving…">Save draft</PendingLabel></button>
        {:else}
          <button class="btn drafts__discard" type="button" disabled data-fid="guide-save">Save draft</button>
          <button class="btn btn--primary btn--key drafts__merge" type="button" data-fid="guide-publish" aria-disabled={blocked !== null} onclick={() => !blocked && (planOpen = true)}><Icon name="message-square" />Publish…</button>
        {/if}
      </div>
    {/if}
  </aside>
{/snippet}

{#snippet editor()}
  <header class="drafts__head" data-fid="guide-head">
    <span class="proposal__art drafts__headart" aria-hidden="true">{#if portrait}<img class="portrait portrait--md" src={portrait} alt="" />{/if}</span>
    <div class="drafts__headtext">
      {#if !bar}<p class="cap">Guide · {target}</p>{/if}
      {#if renaming}
        <div class="drafts__rename">
          <label class="vh" for="{uid}-title">Guide title</label>
          <input
            id="{uid}-title"
            class="drafts__titlefield"
            bind:value={title}
            maxlength="100"
            required
            onkeydown={(event) => {
              if (event.key === 'Enter' || event.key === 'Escape') {
                event.preventDefault();
                if (event.key === 'Escape' && base) title = base.guide.title;
                void stopRename();
              }
            }}
          />
          <button class="btn btn--ghost" type="button" onclick={() => void stopRename()}>Done</button>
        </div>
      {:else}
        <div class="drafts__titlerow" data-fid="guide-title">
          <h2 class="drafts__title">{title || row.title}</h2>
          <button class="btn btn--ghost drafts__renamebtn" type="button" bind:this={renameButton} title="Rename" aria-label="Rename {title || row.title}" onclick={() => void startRename()}><Icon name="edit" /></button>
        </div>
      {/if}
      <p class="drafts__meta" data-fid="guide-meta">{meta}</p>
    </div>
  </header>
  {#if stale}
    <div class="drafts__moved" role="alert" data-fid="guide-stale">
      <Icon name="alert-triangle" />
      <p><b>{who(stale.guide.updated_by)} saved version {stale.guide.version} at {at(stale.guide.updated_at)} while you were editing.</b> Your {plural(changes.length, 'change')} {changes.length === 1 ? 'isn’t' : 'aren’t'} saved. Copy what you need, then load version {stale.guide.version}.</p>
    </div>
    <div class="guide-unknown__actions">
      {#each staleCopies as c (c.n)}<button class="btn" type="button" onclick={() => void copyText(c.text, c.n)}><Icon name="copy" />Copy my block {c.n} text</button>{/each}
      <button class="btn btn--primary" type="button" onclick={loadNewer}><Icon name="refresh-cw" />Load version {stale.guide.version}</button>
    </div>
  {/if}
  {#if unknown}
    <div class="draftcfl guide-unknown" role="group" aria-labelledby="{uid}-unk" data-fid="guide-unknown">
      <div class="draftcfl__head">
        <b id="{uid}-unk">Couldn’t confirm message {unknown.position + 1} was posted</b>
        {#if searching}<StatusChip tone="neutral">Searching…</StatusChip>{:else}<StatusChip tone="risk">Needs attention</StatusChip>{/if}
      </div>
      {#if searching}
        <p class="draftcfl__text">Reading the last 50 messages in <span class="mono">{channelName(unknown.channel_id)}</span>…</p>
      {:else}
        <p class="draftcfl__text">
          Discord didn’t confirm Kanade’s post of message {unknown.position + 1}. Kanade searched <span class="mono">{channelName(unknown.channel_id)}</span> and found no single matching message. So nobody sees it twice, this guide publishes nothing until you choose.
        </p>
        <div class="guide-unknown__actions">
          <button class="btn" type="button" data-fid="guide-unknown-search" onclick={() => void search()}><Icon name="search" />Search again</button>
          <button
            class="btn btn--danger"
            type="button"
            data-fid="guide-unknown-mark"
            onclick={() => {
              markError = '';
              markOpen = true;
            }}>Mark as not posted…</button
          >
        </div>
      {/if}
    </div>
  {:else if found}
    <div class="flash flash--ok gplan__flash" role="status" data-fid="guide-unknown-found"><b>Found it.</b> The message was in the channel; it’s recorded now, and the guide can publish again.</div>
  {/if}
  {#if live && run}
    <p class="drafts__live" data-fid="guide-live-note"><Icon name="clock" /><span><b>Publishing version {run.version}.</b> You can keep editing; changes wait for the next publish.</span></p>
  {:else}
    <p class="drafts__live" data-fid="guide-live-note"><Icon name="eye-off" /><span><b>Saving never posts.</b> Members see changes only after you publish.</span></p>
  {/if}
  <div class="guide-channel" data-fid="guide-channel">
    <span class="cap" id="{uid}-ch">Channel</span>
    <Select bind:value={channel} {options} label="Channel" placeholder="Choose a channel" noun="channels" />
    <span class="guide-channel__note">{wasChannel ? `Was ${wasChannel}. The guide moves at the next publish.` : 'One guide per channel.'}</span>
  </div>
  <GuideBlocks
    bind:blocks
    bind:openId
    saved={savedIds}
    {positions}
    {preview}
    bosses={bossList.data ?? []}
    {problems}
    flags={unknownBlock ? { [unknownBlock]: 'Unconfirmed: posted or not?' } : {}}
  />
  <p class="field__error" role="alert">{saveError}</p>
{/snippet}

{#snippet previewBody()}
  <GuidePreview
    slots={preview?.slots ?? []}
    bot={directory.bot.name}
    at={row.last_run?.finished_at ? at(row.last_run.finished_at).slice(-5) : at(now || new Date().toISOString()).slice(-5)}
    {here}
    tags={{
      ...Object.fromEntries((preview?.slots ?? []).filter((s) => !savedIds.has(s.block_id)).map((s) => [s.position, { tone: 'neutral' as const, text: 'unsaved · new message' }])),
      ...(unknown ? { [unknown.position]: { tone: 'risk' as const, text: 'unconfirmed · posted or not?' } } : {}),
    }}
  />
  {#if previewError}<p class="flash flash--error" role="alert">Couldn’t draw the preview: {previewError}</p>{/if}
  <p class="reminder-pane__note" data-fid="guide-preview-note"><Icon name="info" /><span>{dirty ? 'Your unsaved edits. ' : ''}Mentions show as names; nobody is pinged. Messages post silently.</span></p>
{/snippet}

{#snippet runBody(r: GuideRun)}
  {#if live}
    <WavyProgress value={r.progress.done} max={Math.max(1, runTotal)} label="{r.kind === 'search' ? 'Searching' : 'Publishing'} {title}" text="{r.progress.done} of {runTotal} changes made" />
    <p class="guide-run__note">It carries on if you leave this page. Discord allows about 5 messages every 5 seconds per channel.</p>
  {:else if r.state === 'done'}
    <p class="guide-run__summary"><StatusChip tone="ok"><Icon name="check" />{runSummary(r)}</StatusChip><StatusChip tone="neutral">nobody pinged</StatusChip></p>
  {:else}
    <div class={['flash', r.state === 'cancelled' ? 'flash--warn' : 'flash--warn', 'gplan__flash']} role="status" data-fid="guide-run-stopped">
      {#if r.state === 'cancelled'}<b>Cancelled.</b> {runSummary(r)}; nothing after that was sent. Publish again to finish.{:else}{stopWords(r.stop_reason, r, channelName(r.channel_id))}{/if}
    </div>
  {/if}
  {@render runOps(r)}
{/snippet}

{#if detail.error && !detail.data}
  <p class="flash flash--error drafts__readerror" role="alert">Couldn't load this guide: {detail.error} <button class="btn" type="button" onclick={() => void detail.load()}>Try again</button></p>
{:else if base}
  {#if bar}
    <div class="drafts__view drafts__view--bar guide-phone">
      <div class="seg guide-phonetabs" role="tablist" aria-label={title} data-fid="guide-phone-tabs">
        <button type="button" role="tab" id="{uid}-tab-edit" aria-selected={tab === 'edit'} aria-controls="{uid}-panel" onclick={() => (tab = 'edit')}>Edit</button>
        <button type="button" role="tab" id="{uid}-tab-preview" aria-selected={tab === 'preview'} aria-controls="{uid}-panel" onclick={() => (tab = 'preview')}>Preview · {plural(preview?.slots.length ?? 0, 'message')}</button>
      </div>
      <div class="drafts__main" id="{uid}-panel" role="tabpanel" tabindex="-1" aria-labelledby="{uid}-tab-{tab}" data-fid={tab === 'edit' ? 'guide-editor' : 'guide-preview'}>
        <form class="guide-phone__form" id={editForm} onsubmit={save}>
          {#if tab === 'edit'}{@render editor()}{:else if run && (live || outcomeShown)}{@render runBody(run)}{:else}{@render previewBody()}{/if}
        </form>
      </div>
      {@render phoneBar()}
    </div>
  {:else}
    <div class="drafts__view">
      <form class="guide-edit" id={editForm} onsubmit={save}>
        <div class="drafts__main" data-fid="guide-editor">{@render editor()}</div>
        <SaveBar form={editForm} label="Save draft" {changes} {saving} disabled={over} ondiscard={discard} clean="Draft saved · version {base.guide.version}" fid="guide-savebar" saveFid="guide-save" class="guide-savebar" />
      </form>
      <aside class="side-pane drafts__pane guide-pane" aria-label={run && (live || outcomeShown) ? 'Publishing' : 'Preview and publish'} data-fid="guide-pane">
        <div class="drafts__panehead" data-fid="guide-pane-head">
          {#if run && live}
            <p class="cap">{run.kind === 'search' ? 'Searching' : run.kind === 'unpublish' ? 'Unpublishing' : `Publishing version ${run.version}`} · started {at(run.requested_at)} by {who(run.requested_by)}</p>
            <h3 class="drafts__panetitle">{run.progress.done} of {plural(runTotal, 'change')} made</h3>
          {:else if run && outcomeShown}
            <p class="cap">{run.state === 'done' ? (run.kind === 'unpublish' ? 'Unpublished' : run.kind === 'search' ? 'Searched' : 'Published') : run.state === 'cancelled' ? 'Cancelled' : 'Stopped'} {run.finished_at ? at(run.finished_at) : ''}</p>
            <h3 class="drafts__panetitle">{run.state === 'done' && run.kind === 'publish' ? `${plural(published, 'message')} in ${channelName(run.channel_id)}` : `${run.progress.done} of ${plural(runTotal, 'change')} made`}</h3>
          {:else}
            <p class="cap">Preview · {dirty ? 'your unsaved edits' : `version ${base.guide.version}${row.status === 'published' ? ', as posted' : ''}`}</p>
            <h3 class="drafts__panetitle">{plural(preview?.slots.length ?? 0, 'message')} in {target}</h3>
          {/if}
        </div>
        <!-- Focusable: the preview scrolls and may hold nothing else to tab to (axe scrollable-region-focusable). -->
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <div class="drafts__panebody" tabindex="0" role="region" aria-label={run && (live || outcomeShown) ? 'Publishing progress' : 'Preview'} data-fid={run && (live || outcomeShown) ? 'guide-progress' : 'guide-preview'}>
          {#if run && (live || outcomeShown)}{@render runBody(run)}{:else}{@render previewBody()}{/if}
        </div>
        {@render decision()}
      </aside>
    </div>
  {/if}
{/if}

<PublishPlan
  bind:open={planOpen}
  guide={id}
  title={base?.guide.title ?? row.title}
  {channelName}
  {checkedAt}
  onqueued={started}
  onoffline={(message) => (offline = message)}
  onstale={(message) => {
    toaster.show({ message, tone: 'error' });
    void detail.load();
  }}
/>

<Modal bind:open={markOpen} title="Mark message {(unknown?.position ?? 0) + 1} as not posted?" eyebrow="Guides · {title}" narrow alert describedby="{uid}-mark" fid="guide-unknown-confirm">
  <p id="{uid}-mark">Kanade searched <span class="mono">{unknown ? channelName(unknown.channel_id) : ''}</span> and found no single copy of message {(unknown?.position ?? 0) + 1}. Marking it not posted lets the next publish post it again.</p>
  <div class="flash flash--warn gplan__flash" role="note" data-fid="guide-unknown-warn"><b>If the first try did land,</b> members will see message {(unknown?.position ?? 0) + 1} twice. Delete the extra copy in Discord by hand.</div>
  <p class="note">Kept in History. Kanade never resends it on its own.</p>
  <p class="field__error" role="alert">{markError}</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button class="btn btn--risk" type="button" data-fid="guide-unknown-mark-confirm" disabled={markBusy} onclick={() => void markNotPosted()}><PendingLabel pending={markBusy} label="Marking…">Mark as not posted</PendingLabel></button>
  {/snippet}
</Modal>

<Modal bind:open={deleteOpen} title="Delete {base?.guide.title ?? row.title}?" eyebrow="Guides" narrow alert describedby="{uid}-del" fid={published ? 'guide-delete-dialog' : 'guide-delete-confirm'}>
  {#if published}
    <p id="{uid}-del">{base?.guide.title} still has <b>{plural(published, 'message')}</b> in <span class="mono">{channelName(detail.data?.slots[0]?.channel_id ?? channel)}</span>. Unpublish it first: Kanade deletes {published === 1 ? 'that message' : `those ${published} messages`}, newest first. Then you can delete the guide.</p>
    <p class="note">Members stop seeing the guide as soon as its messages go. The draft stays until you delete it; History keeps the record.</p>
  {:else}
    <p id="{uid}-del">Nothing is posted any more. The draft and its {plural(base?.blocks.length ?? 0, 'block')} go; History keeps the record.</p>
  {/if}
  <p class="field__error" role="alert">{deleteError}</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    {#if published}
      <button class="btn btn--risk" type="button" data-fid="guide-unpublish" disabled={deleteBusy || !unpublishPlan} onclick={() => void confirmDelete()}><PendingLabel pending={deleteBusy} label="Unpublishing…">Unpublish {plural(published, 'message')}</PendingLabel></button>
    {:else}
      <button class="btn btn--risk" type="button" disabled={deleteBusy} onclick={() => void confirmDelete()}><Icon name="trash-2" /><PendingLabel pending={deleteBusy} label="Deleting…">Delete guide</PendingLabel></button>
    {/if}
  {/snippet}
</Modal>
