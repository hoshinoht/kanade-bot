<!--
  One inbox item: v4's evidence quote, the proposed change as a preview with
  any conflicts (which always block), per-run choices for a weekly-timing
  change, and the actions — Approve, Edit then approve (a move, new run or
  split), Reject (with a reason for member requests).
-->
<script lang="ts">
  import type { ApproveRequest, Proposal } from '@kanade/api-types';
  import { BossTag } from '@kanade/ui';
  import { editable } from './edit';
  import { blocked, DISCORD_ONLY, FLAG_LABEL, FLAG_TONE, isProposal, SOURCE_LABEL } from './flags';

  let {
    p,
    busy,
    locked,
    error,
    onapprove,
    onmove,
    onreject,
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
  } = $props();
  const uid = $props.id();

  let edit = $state('');
  let choices = $state<Record<string, 'update' | 'keep'>>({});
  const band = (c: number | null) => (c === null ? 'unknown' : c >= 0.8 ? 'high' : c >= 0.6 ? 'mid' : 'low');
  const stop = $derived(blocked(p));
  const conflicted = $derived(p.preview.conflicts.length > 0);
  const refused = $derived(locked && isProposal(p));

  function approve() {
    const body: ApproveRequest = { version: p.version };
    // A timing change always names its choices, `{}` when no run is listed.
    if (p.choices !== null) body.choices = { ...choices };
    onapprove(body);
  }
</script>

<article class="proposal" aria-labelledby="{uid}-title">
  <header class="proposal__head">
    <h2 class="proposal__title" id="{uid}-title">
      {p.kind_label} — {#each p.bosses as boss (boss.token)}<BossTag {boss} />{/each}
    </h2>
    <span class="chip{p.source === 'self_service' ? ' chip--maybe' : ''}">{SOURCE_LABEL[p.source]}</span>
    {#if p.source !== 'self_service'}
      <span class="conf conf--{band(p.confidence)}">{p.confidence === null ? 'no score' : `${p.confidence.toFixed(2)} confident`}</span>
    {/if}
    {#each p.flags as flag (flag)}<span class="chip {FLAG_TONE[flag]}">{FLAG_LABEL[flag]}</span>{/each}
    {#if p.is_question}<span class="chip chip--waiting">still a question</span>{/if}
    {#if p.channel}<span class="chip">{p.channel}</span>{/if}
    <span class="id">#{p.short_id} · read {p.read_at}</span>
  </header>

  <p class="note"><em>{p.summary}</em></p>

  {#if p.self_service}
    <p class="flash flash--ok">
      Sent by <strong>{p.self_service.member.name}</strong> (#{p.self_service.member.id}) as a member request.
      {#if p.self_service.note}“{p.self_service.note}”{/if}
    </p>
  {/if}

  {#if p.evidence.length}
    <div class="evidence" aria-label="Evidence">
      {#each p.evidence as line (line.id)}
        <p class="evidence__line">
          {#if line.missing}
            <span class="evidence__text">A message from {line.author} is no longer stored.</span>
          {:else}
            <span class="evidence__who">{line.author}</span><span class="evidence__at">{line.at}</span>
            <br /><span class="evidence__text">{line.content}</span>
            {#if line.url}<a class="id" href={line.url} target="_blank" rel="noopener noreferrer">open</a>{/if}
          {/if}
        </p>
      {/each}
    </div>
  {/if}

  <h3 class="pane__section">Would change</h3>
  {#if p.preview.changes.length}
    <ul class="proposal__changes">
      {#each p.preview.changes as change, i (i)}
        <li>
          <span class="proposal__field">{change.field}</span>
          <span class="mono was">{change.from}</span> <span aria-hidden="true">→</span><span class="vh">to</span>
          <strong class="mono">{change.to}</strong>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="note">
      <strong class="mono">{p.when}</strong>
      {#if p.participants.length}— {#each p.participants as person (person.id)}<span class="chip">{person.name}</span> {/each}{/if}
    </p>
  {/if}
  {#if p.preview.no_effect}<p class="note">Already in effect: approving would change nothing.</p>{/if}
  {#if p.public_summary}<p class="note">The member sees: “{p.public_summary}”{#if p.expires_at} · expires <span class="mono">{p.expires_at}</span>{/if}</p>{/if}

  {#if conflicted}
    <div class="proposal__conflicts" role="group" aria-labelledby="{uid}-conflicts">
      <h3 class="pane__section" id="{uid}-conflicts">{isProposal(p) ? 'Changed since it was read' : 'Changed since the member asked'}</h3>
      <ul>
        {#each p.preview.conflicts as c, i (i)}
          <li>{c.field}: it was based on <span class="mono">{c.expected}</span>, it is now <strong class="mono">{c.found}</strong></li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if p.choices?.length}
    <fieldset class="proposal__choices">
      <legend>Runs of this weekly timing</legend>
      {#each p.choices as choice (choice.run_id)}
        <div class="proposal__choice" role="radiogroup" aria-label="{choice.label}, {choice.when}">
          <span>{choice.label} · <span class="mono">{choice.when}</span>{#if choice.amended} <span class="status status--planned">amended</span>{/if}</span>
          <label><input type="radio" name="{uid}-{choice.run_id}" value="update" bind:group={choices[choice.run_id]} /> Update to the new timing</label>
          <label><input type="radio" name="{uid}-{choice.run_id}" value="keep" bind:group={choices[choice.run_id]} /> Keep as it is</label>
        </div>
      {/each}
    </fieldset>
  {/if}

  <div class="proposal__actions">
    <button class="btn btn--primary" type="button" disabled={busy || Boolean(stop) || refused} aria-describedby="{uid}-why" onclick={approve}
      >Approve</button
    >
    {#if editable(p) && !stop}
      <form
        class="proposal__edit"
        onsubmit={(event) => {
          event.preventDefault();
          onmove(edit);
        }}
      >
        <label class="field">
          <span>Edit, then approve</span>
          <input class="mono" bind:value={edit} placeholder="wed 21:30" size="10" aria-invalid={error ? 'true' : undefined} aria-describedby="{uid}-err" />
        </label>
        <button class="btn" type="submit" disabled={busy || refused} aria-describedby="{uid}-why">Move &amp; approve</button>
      </form>
    {/if}
    <button class="btn btn--danger" type="button" disabled={refused} aria-describedby="{uid}-why" onclick={onreject}>Reject…</button>
    {#if p.card_url}<a class="btn btn--ghost" href={p.card_url} target="_blank" rel="noopener noreferrer">See the card</a>{/if}
  </div>
  <p class="note" id="{uid}-why">{refused ? `${DISCORD_ONLY} Members' requests can still be decided here.` : stop}</p>
  <p class="field__error" id="{uid}-err" role="alert">{error}</p>
</article>
