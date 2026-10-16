<!--
  The publish plan (boards G_Plan, G_PlanRepost, G_PlanMove, G_PlanStates,
  G_PhonePlan*): RevertDialog's shape. It names the saved version and what it
  was checked against, the Keep / Edit / Add / Delete counts, the
  confirmations a middle repost, foreign messages below the guide or a
  channel move need (each its own tick; a move's "delete the old messages"
  ticked by default), why it cannot run, the ops in run order (message id
  tails on the desktop only) and that posts are silent. Publish sends the
  plan's hash with one Idempotency-Key per opening, so a double press or a
  retry is one run; a 409 `plan_changed` redraws the new plan, nothing sent.
-->
<script lang="ts">
  import type { GuidePlan, GuidePublishResult, GuideRun } from '@kanade/api-types';
  import { newIdempotencyKey } from '@kanade/client';
  import { Icon, Modal, PendingLabel } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import { OP_WORD, guidePath, keptPositions, listWords, opLabel, plural, tail } from './guides';

  let {
    open = $bindable(false),
    guide,
    title,
    channelName,
    checkedAt,
    onqueued,
    onoffline,
    onstale,
  }: {
    open: boolean;
    guide: string;
    title: string;
    /** `#name` of a channel id (the guide's, or the old one on a move). */
    channelName: (id: string) => string;
    /** The server clock's "12:00", when the plan was read. */
    checkedAt: () => string;
    onqueued: (run: GuideRun) => void;
    /** Discord is offline (503): the pane says so and Publish stays off. */
    onoffline: (message: string) => void;
    /** Someone saved a newer version: the editor reloads. */
    onstale: (message: string) => void;
  } = $props();
  const uid = $props.id();

  let plan = $state<GuidePlan | null>(null);
  let readError = $state('');
  let error = $state('');
  let changed = $state('');
  let noop = $state(false);
  let busy = $state(false);
  let deleteOld = $state(true);
  let repost = $state(false);
  let interleaved = $state(false);
  let checked = $state('');
  // One action per opening: a double press or a network retry replays it.
  let key = newIdempotencyKey();

  async function load(): Promise<boolean> {
    const read = await send((c) => c.get<GuidePlan>(guidePath(guide, `/plan?delete_old=${deleteOld ? 1 : 0}`)));
    if (!read.ok) {
      readError = read.message;
      return false;
    }
    plan = read.value;
    readError = '';
    checked = checkedAt();
    return true;
  }

  $effect(() => {
    if (!open) return;
    plan = null;
    error = '';
    changed = '';
    noop = false;
    deleteOld = true;
    repost = false;
    interleaved = false;
    key = newIdempotencyKey();
    void load();
  });

  const target = $derived(plan ? channelName(plan.channel_id) : '');
  const moveFrom = $derived(plan?.ops.find((op) => op.kind === 'delete' && op.channel_id !== plan?.channel_id)?.channel_id ?? null);
  const oldName = $derived(moveFrom ? channelName(moveFrom) : '');
  const oldCount = $derived(plan && moveFrom ? plan.ops.filter((op) => op.channel_id === moveFrom && (op.kind === 'delete' || op.kind === 'forget')).length : 0);
  const posted = $derived(plan ? plan.counts.keep + plan.counts.edit + plan.ops.filter((op) => op.kind === 'delete' && op.channel_id === plan?.channel_id).length : 0);
  const repostTo = $derived(plan ? Math.max(0, ...plan.ops.filter((op) => op.kind === 'delete' && op.channel_id === plan?.channel_id).map((op) => op.position + 1)) : 0);
  const changes = $derived(plan ? plan.counts.edit + plan.counts.create + plan.counts.delete : 0);
  const kept = $derived(plan ? keptPositions(plan).map((p) => p + 1) : []);
  const confirmed = $derived(Boolean(plan) && (plan!.needs.repost_from === null || repost) && (!plan!.needs.interleaved || interleaved));
  const runnable = $derived(Boolean(plan) && !plan!.blocked && !plan!.no_op && confirmed && !busy);
  const GRANT: Record<string, string> = { view: 'see', send: 'send messages', history: 'read the history', embed: 'embed links', attach: 'attach files' };

  async function toggleOld() {
    deleteOld = !deleteOld;
    key = newIdempotencyKey();
    await load();
  }

  async function publish() {
    if (!plan || !runnable) return;
    busy = true;
    error = '';
    const p = plan;
    const done = await send((c) =>
      c.post<GuidePublishResult>(
        guidePath(guide, '/publish'),
        {
          version: p.version,
          plan_hash: p.plan_hash,
          confirm: { repost_from: repost ? p.needs.repost_from : null, interleaved, channel_move: p.needs.channel_move },
          delete_old: deleteOld,
        },
        { idempotencyKey: key },
      ),
    );
    busy = false;
    if (done.ok) {
      if (done.value.no_op || !done.value.publish) noop = true;
      else {
        open = false;
        onqueued(done.value.publish);
      }
      return;
    }
    if (done.code === 'plan_changed') {
      key = newIdempotencyKey();
      if (await load()) changed = 'The plan changed since you opened it. Nothing was sent. This is the new plan.';
      return;
    }
    if (done.code === 'stale') {
      open = false;
      onstale(done.message);
      return;
    }
    if (done.status === 503) onoffline(done.message);
    error = done.message;
  }
</script>

<Modal bind:open title={plan?.needs.channel_move ? `Move ${title} to ${target}` : `Publish ${title}`} eyebrow="Guides · publish plan" className="gplan" fid="guide-plan">
  {#if readError}
    <p class="flash flash--error gplan__flash" role="alert">Couldn’t read the plan: {readError}</p>
  {:else if !plan}
    <p class="note" aria-busy="true">Checking the channel…</p>
  {:else}
    {#if changed}<div class="flash flash--error gplan__flash" role="alert" data-fid="guide-plan-changed"><b>{changed}</b></div>{/if}
    <p class="gplan__lead">
      {#if plan.needs.channel_move}Version {plan.version}. The guide is in <span class="mono">{oldName}</span> now and moves to <span class="mono">{target}</span>
      {:else if posted}Version {plan.version} against the {plural(posted, 'message')} in <span class="mono">{target}</span> now
      {:else}Version {plan.version}, first publish to <span class="mono">{target}</span>{/if}{#if plan.checked} · checked {checked}.{:else}. Not checked against Discord.{/if}
    </p>
    {#if plan.no_op || noop}
      <div class="flash flash--ok gplan__flash" role="status" data-fid="guide-plan-noop"><b>Nothing to publish.</b> Discord already shows version {plan.version}.</div>
    {:else}
      <p class="gplan__counts" data-fid="guide-plan-counts">
        {#each [['keep', plan.counts.keep], ['edit', plan.counts.edit], ['add', plan.counts.create], ['delete', plan.counts.delete]] as const as [k, n] (k)}
          <span class="chip chip--mono gplan__count gplan__count--{k}" class:gplan__count--zero={!n}>{k === 'add' ? 'Add' : OP_WORD[k]} <b>{n}</b></span>
        {/each}
      </p>
      {#if !plan.checked}
        <div class="flash flash--warn gplan__flash" role="note" data-fid="guide-plan-warn">
          <b>Not checked:</b> Discord didn’t answer within 5 seconds, so this assumes every posted message is still there. Kanade checks again before it sends anything and stops if the plan differs.
        </div>
      {/if}
      {#if plan.needs.repost_from !== null}
        <div class="drafts__moved gplan__confirm" role="group" aria-labelledby="{uid}-repost" data-fid="guide-plan-confirm">
          <Icon name="alert-triangle" />
          <div class="gplan__confirmtext">
            <p id="{uid}-repost"><b>Messages {plan.needs.repost_from + 1}–{repostTo} will be deleted and posted again.</b> Members may see them disappear and reappear. Discord can’t change a posted message’s images by editing it, and the guide keeps its order.</p>
            <label class="ack"><input type="checkbox" bind:checked={repost} /> <span>Delete and repost messages {plan.needs.repost_from + 1}–{repostTo}</span></label>
          </div>
        </div>
      {/if}
      {#if plan.needs.interleaved}
        <div class="drafts__moved gplan__confirm" role="group" aria-labelledby="{uid}-inter" data-fid="guide-plan-confirm">
          <Icon name="alert-triangle" />
          <div class="gplan__confirmtext">
            <p id="{uid}-inter"><b>Newer messages sit below the guide in {target}.</b> The new messages will appear after them, not right under the guide.</p>
            <label class="ack"><input type="checkbox" bind:checked={interleaved} /> <span>Post below the newer messages</span></label>
            <p class="gplan__after">A channel only Kanade can post in avoids this.</p>
          </div>
        </div>
      {/if}
      {#if plan.needs.channel_move}
        <div class="drafts__moved gplan__confirm" role="group" aria-labelledby="{uid}-move" data-fid="guide-plan-confirm">
          <Icon name="alert-triangle" />
          <div class="gplan__confirmtext">
            <p id="{uid}-move"><b>Moving to {target}.</b> Kanade posts all {plan.counts.create} messages there first, then deletes the old ones only after every new message is confirmed.</p>
            <label class="ack"><input type="checkbox" checked={deleteOld} disabled={busy} onchange={() => void toggleOld()} /> <span>Then delete the {plural(oldCount, 'old message')} in {oldName}</span></label>
            <p class="gplan__after">Untick to leave the old messages in {oldName}; Kanade stops tracking them and they stay until someone deletes them by hand.</p>
          </div>
        </div>
      {/if}
      {#if plan.blocked}
        <div class="flash flash--error gplan__flash" role="alert" data-fid="guide-plan-grants">
          {#if plan.blocked.reason === 'unknown_slot'}<b>Message {(plan.blocked.position ?? 0) + 1} may or may not have posted.</b> Settle it first: Search again, or Mark as not posted.
          {:else if plan.blocked.reason === 'channel_missing'}<b>{target} is gone, or Kanade can’t see it.</b> Choose another channel, save, then open the plan again.
          {:else}<b>Kanade can’t {listWords(plan.blocked.grants.map((g) => GRANT[g] ?? g))} in {target}.</b> Allow it for Kanade in that channel, then open the plan again.{/if}
        </div>
      {/if}
      {#each plan.warnings as w, i (i)}
        <div class="flash flash--warn gplan__flash" role="note" data-fid="guide-plan-warn"><b>No {w.art === 'entry' ? 'entry art' : 'portrait'} for {w.key}.</b> Its embed posts without that picture.</div>
      {/each}
      <h3 class="pane__section gplan__head">In this order</h3>
      <ol class="draftops gplan__ops" aria-label="Changes, in the order Kanade makes them">
        {#each plan.ops as op, i (i)}
          {@const kind = op.kind === 'create' ? 'add' : op.kind === 'forget' ? 'keep' : op.kind}
          <li class="draftop gplan__op gplan__op--{kind}" data-fid="guide-plan-op">
            <span class="draftop__ord mono">{op.position + 1}</span>
            <span class="draftop__kind">{OP_WORD[op.kind]}</span>
            <div class="draftop__text">
              <span class="draftop__what"><b>{opLabel(op)}</b></span>
              {#if op.channel_id !== plan.channel_id}<span>In {channelName(op.channel_id)}{op.kind === 'delete' ? ', after every new message is posted' : ''}.</span>
              {:else if op.kind === 'forget'}<span>Gone from Discord already; Kanade stops tracking it.</span>
              {:else if op.kind === 'delete' && i === plan.ops.findIndex((o) => o.kind === 'delete')}<span>Newest first.</span>{/if}
            </div>
            <span class="draftop__at mono">{op.kind === 'create' ? 'new' : tail(op.message_id)}</span>
          </li>
        {/each}
        {#if kept.length && !plan.needs.channel_move}
          <li class="draftop gplan__op gplan__op--keep" data-fid="guide-plan-op">
            <span class="draftop__ord mono">–</span>
            <span class="draftop__kind">Keep</span>
            <div class="draftop__text"><span class="draftop__what"><b>Message{kept.length === 1 ? '' : 's'} {listWords(kept)}</b></span><span>Already as posted.</span></div>
            <span class="draftop__at mono"></span>
          </li>
        {/if}
      </ol>
      <p class="note gplan__silent"><Icon name="bell-off" /><span>Posts silently with no mentions; nobody is pinged.{plan.counts.edit ? ' Edited messages show “(edited)” in Discord.' : ''}</span></p>
    {/if}
    <p class="field__error" role="alert">{error}</p>
  {/if}
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>{plan?.no_op || noop ? 'Close' : 'Cancel'}</button>
    {#if plan && !plan.no_op && !noop}
      <button class="btn btn--primary" type="button" data-fid="guide-plan-publish" aria-disabled={!runnable} onclick={() => void publish()}
        ><PendingLabel pending={busy} label="Publishing…">{plan.needs.channel_move ? `Move and publish ${plural(plan.counts.create, 'message')}` : `Publish ${plural(changes, 'change')}`}</PendingLabel></button
      >
    {/if}
  {/snippet}
</Modal>
