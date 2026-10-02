<!--
  One inbox item (boards B_InboxSelf, B_PhoneInbox): the boss art and a line
  of facts, the member's words, v4's evidence as a thread, the proposed change as a preview with
  any conflicts (which always block), per-run choices for a weekly-timing
  change, and the actions — Approve, Edit then approve (a move, new run or
  split), Reject (with a reason for member requests).
-->
<script lang="ts">
  import type { ApproveRequest, Evidence, Proposal } from '@kanade/api-types';
  import { BossTag, DecisionCard, Icon, PendingLabel, Portrait, ThreadPanel } from '@kanade/ui';
  import { directory } from '../names/directory.svelte';
  import Mentions from '../names/Mentions.svelte';
  import Name from '../names/Name.svelte';
  import { editable } from './edit';
  import { blocked, DISCORD_ONLY, isProposal, SOURCE_LABEL } from './flags';

  let {
    p,
    busy,
    locked,
    error,
    onapprove,
    onmove,
    onreject,
    bar = false,
  }: {
    p: Proposal;
    busy: boolean;
    /** This session was refused Kanade's proposals (not signed in with Discord). */
    locked: boolean;
    error: string;
    onapprove: (body: ApproveRequest) => void;
    /** "Move & approve": the typed slot, parsed by the page. */
    onmove: (text: string) => void;
    onreject: () => void;
    /** Narrow frames (≤ 899 px): the decision is the bottom action bar. */
    bar?: boolean;
  } = $props();
  const uid = $props.id();

  let edit = $state('');
  let editOpen = $state(false);
  // Which action the shared `busy` belongs to, so only that button shows it.
  let via = $state<'approve' | 'move'>('approve');
  let choices = $state<Record<string, 'update' | 'keep'>>({});
  const band = (c: number | null) => (c === null ? 'unknown' : c >= 0.8 ? 'high' : c >= 0.6 ? 'mid' : 'low');
  const stop = $derived(blocked(p));
  const conflicted = $derived(p.preview.conflicts.length > 0);
  const refused = $derived(locked && isProposal(p));

  // What each change field is called on screen ("Would change · participants").
  const FIELD: Record<string, string> = { slot: 'slot', participants: 'participants', day_time: 'weekly time', new_fixed: 'new weekly timing', new_run: 'new run' };
  const fieldName = (field: string) => FIELD[field] ?? field.replaceAll('_', ' ');
  /** A participants change as chips: kept, removed (struck) and added names, from the two lists. */
  function roster(from: string, to: string) {
    const list = (text: string) => text.split(',').map((n) => n.trim()).filter((n) => n && n !== '—');
    const before = list(from);
    const after = list(to);
    return [
      ...before.map((name) => ({ name, state: after.includes(name) ? 'kept' : 'removed' })),
      ...after.filter((name) => !before.includes(name)).map((name) => ({ name, state: 'added' })),
    ];
  }

  // The thread (B_PhoneInbox): the channel thread around the evidence, each
  // message marked `used` when the proposal cites it; without one (member
  // requests, older items, an empty thread) the evidence alone, all of it
  // used. The server leaves deleted or pruned messages out of the thread, but
  // the evidence still names them as missing: they are merged back in, used,
  // so a cited message never silently disappears. The Used/All toggle shows
  // when some messages are not used, and starts on Used (on All when none is).
  type Message = Evidence & { used?: boolean };
  const messages = $derived<Message[]>(p.thread?.length ? withGone(p.thread, p.evidence) : p.evidence);
  const usedCount = $derived(messages.filter((m) => m.used !== false).length);
  let showAllPicked = $state<boolean | null>(null);
  const showAll = $derived(showAllPicked ?? usedCount === 0);
  const shownMessages = $derived(showAll ? messages : messages.filter((m) => m.used !== false));
  const initial = (name: string) => [...name.trim()][0]?.toUpperCase() ?? '?';

  const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  /** A sortable minute from a message's `at` ("Mon 28 Sep 21:00"), or null when it has no such date. */
  function minuteOf(at: string): number | null {
    const m = /(\d{1,2}) ([A-Z][a-z]{2})\w* (\d{1,2}):(\d{2})/.exec(at);
    const month = m ? MONTHS.indexOf(m[2]!) : -1;
    return m && month >= 0 ? ((month * 31 + Number(m[1])) * 24 + Number(m[3])) * 60 + Number(m[4]) : null;
  }
  const snowflake = (id: string) => (/^\d{15,20}$/.test(id) ? BigInt(id) : null);
  /** Whether `a` was sent before `b`: by time, then by Discord id; unknown order is "no". */
  function before(a: Message, b: Message): boolean {
    const [x, y] = [minuteOf(a.at), minuteOf(b.at)];
    if (x !== null && y !== null && x !== y) return x < y;
    const [i, j] = [snowflake(a.id), snowflake(b.id)];
    return i !== null && j !== null && i < j;
  }
  /** The thread plus the cited messages it lacks because they are gone, each in its place in time. */
  function withGone(thread: Message[], evidence: Evidence[]): Message[] {
    const out = [...thread];
    for (const gone of evidence.filter((e) => e.missing && !thread.some((m) => m.id === e.id))) {
      const line: Message = { ...gone, used: true };
      const at = out.findIndex((m) => before(line, m));
      out.splice(at < 0 ? out.length : at, 0, line);
    }
    return out;
  }

  function approve() {
    const body: ApproveRequest = { version: p.version };
    // A timing change always names its choices, `{}` when no run is listed.
    if (p.choices !== null) body.choices = { ...choices };
    via = 'approve';
    onapprove(body);
  }
</script>

<article class="proposal" aria-labelledby="{uid}-title">
  <header class="proposal__head" data-fid="inbox-head">
    {#if p.bosses[0]}<span class="proposal__art" data-fid="inbox-avatar" aria-hidden="true"><Portrait boss={p.bosses[0]} size="md" /></span>{/if}
    <div class="proposal__headtext">
      <h2 class="proposal__title" data-fid="inbox-title" id="{uid}-title">
        {p.kind_label} — {#each p.bosses as boss (boss.token)}<BossTag {boss} />{/each}
      </h2>
      <!-- One line of facts (truncated, never wrapped, on a phone). -->
      <p class="proposal__meta" data-fid="inbox-meta">
        <span class="chip proposal__source">{SOURCE_LABEL[p.source]}</span>
        {#if p.source !== 'self_service'}
          <span class="conf conf--{band(p.confidence)}">{p.confidence === null ? 'no score' : `${p.confidence.toFixed(2)} confident`}</span>
        {/if}
        <span class="proposal__fact mono">#{p.short_id}</span>
        <span class="proposal__fact">read <span class="mono">{p.read_at}</span></span>
        {#if p.channel}<span class="proposal__fact">{p.channel}</span>{/if}
        {#if p.card_url}<a class="proposal__fact proposal__card" href={p.card_url} target="_blank" rel="noopener noreferrer">See the card</a>{/if}
      </p>
    </div>
  </header>

  <div class="proposal__workspace">
  <ThreadPanel label="Proposal thread and changes">

  {#if p.self_service}
    <!-- The member's own words as a speech bubble (mockup `.bub2`). -->
    <div class="proposal__bubble" data-fid="inbox-quote">
      <p class="cap">Sent by <Name kind="member" id={p.self_service.member.id} name={p.self_service.member.name} /> <span class="vh">as a member request.</span></p>
      {#if p.self_service.note}<p class="proposal__said">“{p.self_service.note}”</p>{/if}
    </div>
  {/if}

  <!-- What would change: one card (mockup `.scard`), then what changed since. -->
  <div class="proposal__would" data-fid="inbox-change">
    <h3 class="cap proposal__cap">Would change{#if p.preview.changes.length === 1}<span class="proposal__capfield">· {fieldName(p.preview.changes[0]!.field)}</span>{/if}</h3>
    {#if p.preview.changes.length}
      <ul class="proposal__changes">
        {#each p.preview.changes as change, i (i)}
          <li>
            {#if p.preview.changes.length > 1}<span class="proposal__field">{fieldName(change.field)}</span>{/if}
            {#if change.field === 'participants'}
              <span class="proposal__people">
                {#each roster(change.from, change.to) as person, n (n)}
                  {#if person.state === 'removed'}<del class="proposal__person proposal__person--out">{person.name}<span class="vh"> (leaves)</span></del>
                  {:else if person.state === 'added'}<ins class="proposal__person proposal__person--in">{person.name}<span class="vh"> (joins)</span></ins>
                  {:else}<span class="proposal__person">{person.name}</span>{/if}
                {/each}
              </span>
            {:else}
              <span class="proposal__slot">
                {#if change.from && change.from !== '—'}<span class="mono was">{change.from}</span> <span aria-hidden="true">→</span><span class="vh">to</span>{/if}
                <strong class="mono proposal__to">{change.to}</strong>
              </span>
            {/if}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="note">
        <strong class="mono">{p.when}</strong>
        {#if p.participants.length}<span class="proposal__people">{#each p.participants as person (person.id)}<span class="proposal__person"><Name kind="member" id={person.id} name={person.name} /></span>{/each}</span>{/if}
      </p>
    {/if}
    {#if p.preview.no_effect}<p class="note">Already in effect: approving would change nothing.</p>{/if}
    {#if p.public_summary}<p class="note">The member sees: “{p.public_summary}”{#if p.expires_at} · expires <span class="mono">{p.expires_at}</span>{/if}</p>{/if}
  </div>

  {#if conflicted}
    <div class="proposal__conflicts" data-fid="inbox-risk" role="group" aria-labelledby="{uid}-conflicts">
      <h3 class="cap proposal__cap" id="{uid}-conflicts">{isProposal(p) ? 'Changed since it was read' : 'Changed since the member asked'}</h3>
      {#each p.preview.conflicts as c, i (i)}
        <p>{c.field}: it was based on <span class="mono">{c.expected}</span>, it is now <span class="mono">{c.found}</span>.</p>
      {/each}
    </div>
  {/if}

  {#if p.choices?.length}
    <fieldset class="proposal__choices">
      <legend>Runs of this weekly timing</legend>
      {#each p.choices as choice (choice.run_id)}
        <div class="proposal__choice" role="radiogroup" aria-label="{choice.label}, {choice.when}">
          <span>{choice.label} · <span class="mono">{choice.when}</span>{#if choice.amended} <span class="tone tone--warning">amended</span>{/if}</span>
          <label><input type="radio" name="{uid}-{choice.run_id}" value="update" bind:group={choices[choice.run_id]} /> Update to the new timing</label>
          <label><input type="radio" name="{uid}-{choice.run_id}" value="keep" bind:group={choices[choice.run_id]} /> Keep as it is</label>
        </div>
      {/each}
    </fieldset>
  {/if}

  {#if messages.length}
    <section class="proposal__thread" data-fid="phone-thread" aria-labelledby="{uid}-thread">
      <div class="proposal__threadhead" data-fid="phone-thread-bar">
        <h3 class="proposal__threadtitle" id="{uid}-thread">Thread <span class="mono">· {messages.length} message{messages.length === 1 ? '' : 's'}</span></h3>
        {#if usedCount < messages.length}
          <div class="seg" role="group" aria-label="Messages shown">
            <button type="button" aria-pressed={!showAll} onclick={() => (showAllPicked = false)}>Used {usedCount}</button>
            <button type="button" aria-pressed={showAll} onclick={() => (showAllPicked = true)}>All</button>
          </div>
        {/if}
      </div>
      <ul class="evidence" aria-label="Evidence">
        {#each shownMessages as line (line.id)}
          {@const who = line.author_id ? null : directory.label('member', '', line.author)}
          <li class="msg" data-fid="phone-thread-msg" class:msg--used={line.used !== false} class:msg--gone={line.missing}>
            <span class="msg__av" aria-hidden="true">{initial(who ?? line.author)}</span>
            <div class="msg__body">
              <p class="msg__line">
                <span class="msg__who">{#if line.author_id}<Name kind="member" id={line.author_id} name={line.author} />{:else}{who}{/if}</span>
                <!-- The time opens the message in Discord (no separate "open" link, as on the board). -->
                {#if line.url && !line.missing}<a class="msg__at" href={line.url} target="_blank" rel="noopener noreferrer">{line.at}<span class="vh"> (open in Discord)</span></a>
                {:else}<span class="msg__at">{line.at}</span>{/if}
                {#if line.used !== false}<span class="vh">(used)</span>{/if}
              </p>
              {#if line.missing}<p class="msg__text">This message is no longer stored.</p>
              {:else}<p class="msg__text"><Mentions text={line.content ?? ''} /></p>{/if}
            </div>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
  </ThreadPanel>

  <!-- One set of controls, in the order each layout shows them (DOM order is
       focus order): wide, Approve with its reason above the edit field and
       Reject at the foot; the action bar (≤ 899 px), the reason and the edit
       field first, then pencil → Reject… → Approve (B_PhoneInbox). -->
  {#snippet approveKey()}
    <button class="btn btn--primary btn--key decision__approve" data-fid="decision-approve" type="button" disabled={busy || Boolean(stop) || refused} aria-describedby="{uid}-why" onclick={approve}
      ><Icon name="check" /><PendingLabel pending={busy && via === 'approve'} label="Approving…">Approve</PendingLabel></button
    >
  {/snippet}
  {#snippet why()}
    <p class="decision__why" data-fid="decision-note" id="{uid}-why">
      {#if refused}{DISCORD_ONLY} Members' requests can still be decided here.
      {:else if stop}<strong>Can’t approve:</strong> {stop}{/if}
    </p>
  {/snippet}
  {#snippet editForm()}
    {#if editable(p) && !stop}
      <form
        id="{uid}-edit"
        class:proposal__edit--open={editOpen}
        class="proposal__edit"
        onsubmit={(event) => {
          event.preventDefault();
          via = 'move';
          onmove(edit);
        }}
      >
        <label class="field">
          <span>Edit, then approve</span>
          <input class="mono" bind:value={edit} placeholder="wed 21:30" size="10" aria-invalid={error ? 'true' : undefined} aria-describedby="{uid}-err" />
        </label>
        <button class="btn" type="submit" disabled={busy || refused} aria-describedby="{uid}-why"
          ><PendingLabel pending={busy && via === 'move'} label="Approving…">Move &amp; approve</PendingLabel></button
        >
      </form>
    {/if}
  {/snippet}
  {#snippet rejectKey()}
    <!-- When Approve is blocked, Reject becomes the key action (risk fill). -->
    <button class="btn btn--danger decision__reject" data-fid="decision-reject" class:btn--risk={Boolean(stop) && !refused} class:btn--key={Boolean(stop) && !refused} type="button" disabled={refused} aria-describedby="{uid}-why" onclick={onreject}>Reject…</button>
  {/snippet}

  <DecisionCard label="Decide this change">
    {#if bar}
      {@render why()}
      {@render editForm()}
      {#if editable(p) && !stop}
        <!-- The pencil opens the edit field above the bar. -->
        <button
          class="btn proposal__edit-toggle"
          type="button"
          aria-label="Show the edit field"
          aria-expanded={editOpen}
          aria-controls="{uid}-edit"
          onclick={() => (editOpen = !editOpen)}><Icon name="edit" /></button
        >
      {/if}
      {@render rejectKey()}
      {@render approveKey()}
    {:else}
      {@render approveKey()}
      {@render why()}
      {@render editForm()}
      <span class="decision__spacer" aria-hidden="true"></span>
      {@render rejectKey()}
      <p class="decision__foot" data-fid="decision-foot">{p.self_service ? 'Reject tells the member, with your reason.' : 'Reject marks the card in Discord as rejected.'}</p>
    {/if}
    <p class="field__error" id="{uid}-err" role="alert">{error}</p>
  </DecisionCard>
  </div>
</article>
