<!--
  One inbox item (boards B_InboxSelf, B_PhoneInbox): the boss art and a line
  of facts, the member's words, v4's evidence as a thread, the proposed change as a preview with
  any conflicts (which always block), per-run choices for a weekly-timing
  change, and the actions — Approve, Edit then approve (a move, new run or
  split), Reject (with a reason for member requests).
-->
<script lang="ts">
  import type { ApproveRequest, Evidence, Proposal, RunStatus } from '@kanade/api-types';
  import { BossTag, DecisionCard, Icon, initial, PendingLabel, Portrait, RUN_TONE, STATUS_WORDS, StatusChip, ThreadPanel } from '@kanade/ui';
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
  /** Wide Extractor items (VarRail2): header across, thread beside the decision card. */
  const stacked = $derived(!bar && isProposal(p));
  const confText = $derived(p.confidence === null ? 'no score' : `${p.confidence.toFixed(2)} confident`);
  /** The reason Approve is held back, when it is: describes the actions. */
  const whyId = $derived(refused || stop ? `${uid}-why` : undefined);

  // What each change field is called on screen ("Would change · participants").
  const FIELD: Record<string, string> = { slot: 'slot', participants: 'participants', day_time: 'weekly time', new_fixed: 'new weekly timing', new_run: 'new run' };
  const fieldName = (field: string) => FIELD[field] ?? field.replaceAll('_', ' ');
  /** A run status as its word and chip tone ("at_risk" → "At risk", risk); unknown values stay as sent. */
  const CHIP_TONE = { success: 'ok', danger: 'risk', warning: 'warn', info: 'neutral', neutral: 'neutral' } as const;
  function statusChip(value: string) {
    const known = value in STATUS_WORDS ? (value as RunStatus) : null;
    const word = known ? STATUS_WORDS[known] : value.replaceAll('_', ' ');
    return { word: word.charAt(0).toUpperCase() + word.slice(1), tone: known ? CHIP_TONE[RUN_TONE[known]] : ('neutral' as const) };
  }
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
  /** The thread's time span, "Mon 28 Sep 21:00–22:00" when it is one day. */
  const span = $derived.by(() => {
    const ats = messages.map((m) => m.at).filter(Boolean);
    const [a, b] = [ats[0], ats.at(-1)];
    if (!a || !b || a === b) return a ?? '';
    const split = (at: string) => [at.slice(0, at.lastIndexOf(' ')), at.slice(at.lastIndexOf(' ') + 1)];
    const [da, ta] = split(a);
    const [db, tb] = split(b);
    return da === db ? `${da} ${ta}–${tb}` : `${a} – ${b}`;
  });
  /** Where the conversation is in Discord: the first used message that still exists. */
  const discordUrl = $derived(messages.find((m) => m.used !== false && m.url && !m.missing)?.url ?? null);
  /** "Wed 30 Sep 23:30" as its date and its time, so the decision card can set the time under the date. */
  const clock = (text: string) => /^(.+) (\d{1,2}:\d{2})$/.exec(text)?.slice(1, 3) as [string, string] | undefined;

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

<!-- Two layouts share these parts. Self-service items and narrow frames
     (B_InboxSelf, B_PhoneInbox): the item's column beside the decision pane.
     Wide Extractor items (VarRail2): the header across the top, then the
     thread panel beside a 300 px decision card that holds the change. -->
{#snippet facts()}
  <!-- One line of facts (truncated, never wrapped, on a phone). -->
  <p class="proposal__meta" data-fid="inbox-meta">
    <span class="chip proposal__source">{SOURCE_LABEL[p.source]}</span>
    {#if p.source !== 'self_service' && !stacked}
      <span class="conf conf--{band(p.confidence)}">{confText}</span>
    {/if}
    <span class="proposal__fact proposal__fact--aside mono">#{p.short_id}</span>
    <span class="proposal__fact proposal__fact--aside">read {p.read_at}</span>
    {#if p.channel && !(stacked && messages.length)}<span class="proposal__fact">{p.channel}</span>{/if}
    {#if p.card_url && !stacked}<a class="proposal__fact proposal__card" href={p.card_url} target="_blank" rel="noopener noreferrer">See the card</a>{/if}
  </p>
{/snippet}

{#snippet consequence()}
  <!-- What approving does beyond the change itself (VarRail2), only when the API says. -->
  {#if p.consequence}<p class="proposal__consequence" data-fid="decision-consequence">{p.consequence}</p>{/if}
{/snippet}

{#snippet would()}
  <!-- What would change: one card (mockup `.scard`); in the wide Extractor decision card, its head. -->
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
            {:else if change.field === 'status'}
              <!-- A status change as two chips in the run-status words, never the raw value. -->
              <span class="proposal__status">
                {#if change.from && change.from !== '—'}<del class="proposal__wasstatus"><StatusChip tone="neutral">{statusChip(change.from).word}</StatusChip></del> <span aria-hidden="true">→</span><span class="vh">to</span>{/if}
                <StatusChip tone={statusChip(change.to).tone}>{statusChip(change.to).word}</StatusChip>
              </span>
            {:else}
              <span class="proposal__slot">
                {#if change.from && change.from !== '—'}<span class="mono was">{change.from}</span> <span aria-hidden="true">→</span><span class="vh">to</span>{/if}
                <strong class="mono proposal__to">{#if clock(change.to)}<span class="proposal__todate">{clock(change.to)![0]}</span> {clock(change.to)![1]}{:else}{change.to}{/if}</strong>
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
    <!-- Outside the decision card the change card holds it, never bare text on the ground. -->
    {#if !stacked}{@render consequence()}{/if}
  </div>
  {#if stacked}{@render consequence()}{/if}

{/snippet}

  {#snippet approveKey()}
    <button class="btn btn--primary btn--key decision__approve" data-fid="decision-approve" type="button" disabled={busy || Boolean(stop) || refused} aria-describedby={whyId} onclick={approve}
      ><Icon name="check" /><PendingLabel pending={busy && via === 'approve'} label="Approving…">Approve</PendingLabel></button
    >
  {/snippet}
  {#snippet why()}
    <!-- Only when Approve is held back (refused or blocked). -->
    {#if whyId}
      <p class="decision__why" data-fid="decision-note" id={whyId}>
        {#if refused}{DISCORD_ONLY} Members' requests can still be decided here.
        {:else}<strong>Can’t approve:</strong> {stop}{/if}
      </p>
    {/if}
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
        <button class="btn" type="submit" disabled={busy || refused} aria-describedby={whyId}
          ><PendingLabel pending={busy && via === 'move'} label="Approving…">Move &amp; approve</PendingLabel></button
        >
      </form>
    {/if}
  {/snippet}
  {#snippet rejectKey()}
    <!-- When Approve is blocked, Reject becomes the key action (risk fill). -->
    <button class="btn btn--danger decision__reject" data-fid="decision-reject" class:btn--risk={Boolean(stop) && !refused} class:btn--key={Boolean(stop) && !refused} type="button" disabled={refused} aria-describedby={whyId} onclick={onreject}>Reject…</button>
  {/snippet}

<article class="proposal" class:proposal--stacked={stacked} aria-labelledby="{uid}-title">
  <!-- The item's column (header, then the thread panel). -->
  <div class="proposal__main" data-fid="inbox-detail">
  <header class="proposal__head" data-fid="inbox-head">
    {#if p.bosses[0]}<span class="proposal__art" data-fid="inbox-avatar" aria-hidden="true"><Portrait boss={p.bosses[0]} size="md" /></span>{/if}
    <div class="proposal__headtext">
      <h2 class="proposal__title" data-fid="inbox-title" id="{uid}-title">
        {p.kind_label} — {#each p.bosses as boss (boss.token)}<BossTag {boss} />{/each}
      </h2>
      {#if stacked}
        <!-- What Kanade read; the facts go to the thread's foot (or stay here without a thread). -->
        {#if p.summary}<p class="proposal__summary" data-fid="inbox-summary">{p.summary}</p>{/if}
        {#if !messages.length}{@render facts()}{/if}
      {:else}
        {@render facts()}
      {/if}
    </div>
    {#if stacked}
      <!-- The confidence as the burst badge (VarRail2), in words for screen readers. -->
      <span class="proposal__burst proposal__burst--{band(p.confidence)}" data-fid="inbox-conf"
        ><span class="mono" aria-hidden="true">{p.confidence === null ? '–' : p.confidence.toFixed(2).replace(/^0/, '')}</span><span class="vh">{confText}</span></span
      >
    {/if}
  </header>

  {#snippet panel()}
  <ThreadPanel label="Proposal thread and changes">

  {#if p.self_service}
    <!-- The member's own words as a speech bubble (mockup `.bub2`). -->
    <div class="proposal__bubble" data-fid="inbox-quote">
      <p class="cap">Sent by <Name kind="member" id={p.self_service.member.id} name={p.self_service.member.name} /> <span class="vh">as a member request.</span></p>
      {#if p.self_service.note}<p class="proposal__said">“{p.self_service.note}”</p>{/if}
    </div>
  {/if}

  {#if !stacked}{@render would()}{/if}

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
        {#if stacked}
          {#if p.channel}<span class="proposal__threadfact">{p.channel}</span>{/if}
          {#if span}<span class="proposal__threadfact proposal__threadspan mono">{span}</span>{/if}
        {/if}
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
                {#if line.used !== false}<span class="vh">(used)</span>{#if stacked}<span class="cap msg__used" aria-hidden="true">used</span>{/if}{/if}
              </p>
              {#if line.missing}<p class="msg__text">This message is no longer stored.</p>
              {:else}<p class="msg__text"><Mentions text={line.content ?? ''} /></p>{/if}
            </div>
          </li>
        {/each}
      </ul>
      {#if stacked}
        <div class="proposal__threadfoot" data-fid="phone-thread-foot">
          {@render facts()}
          {#if discordUrl}<a class="proposal__discord" href={discordUrl} target="_blank" rel="noopener noreferrer">Open in Discord</a>{/if}
        </div>
      {/if}
    </section>
  {/if}
  </ThreadPanel>
  {/snippet}

  {#if stacked}
    <div class="proposal__row">
      {@render panel()}
      <DecisionCard label="Decide this change" overline="">
        {@render would()}
        {@render approveKey()}
        {@render why()}
        {@render editForm()}
        <span class="decision__spacer" aria-hidden="true"></span>
        <div class="decision__footrow" data-fid="decision-foot">
          {#if p.card_url}<a class="proposal__card" href={p.card_url} target="_blank" rel="noopener noreferrer">See the card</a>{/if}
          {@render rejectKey()}
        </div>
        <p class="field__error" id="{uid}-err" role="alert">{error}</p>
      </DecisionCard>
    </div>
  {:else}
    {@render panel()}
  {/if}
  </div>

  {#if !stacked}
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
  {/if}
</article>
