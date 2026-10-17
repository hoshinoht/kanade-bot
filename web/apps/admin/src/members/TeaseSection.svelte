<!--
  Members sheet › Profile: Reminder teasing (boards B_MemberTease,
  B_MemberTeaseRefused). The opt-in switch applies at once, like the sheet's
  other controls; the running-joke note saves with its own Save, enabled once
  the text changes, and a refusal stays under the field with the typed text
  kept. Both land in History as separate rows.
-->
<script lang="ts">
  import type { MemberPatch, MemberRow } from '@kanade/api-types';
  import type { Outcome } from '@kanade/ui';
  import { NOTE_MAX, refusalWords, type KnownName } from './tease';

  let {
    member,
    name,
    names,
    busy,
    patch,
  }: {
    member: MemberRow;
    /** The member as the sheet names them. */
    name: string;
    /** Names a note must not carry that the sheet knows (the server checks every one). */
    names: KnownName[];
    busy: boolean;
    /** The sheet's write; `inline` keeps a refusal out of the sheet's notice line. */
    patch: (change: MemberPatch, done: string, inline?: boolean) => Promise<Outcome<MemberRow>>;
  } = $props();
  const uid = $props.id();

  const saved = $derived(member.running_joke ?? '');
  let draft = $state('');
  let error = $state('');
  let seen: string | null = null;
  $effect(() => {
    // Another member, or a save (here or elsewhere), replaces what the field shows.
    const key = `${member.id}\u{1f}${saved}`;
    if (key !== seen) {
      seen = key;
      draft = saved;
      error = '';
    }
  });
  const changed = $derived(draft.trim() !== saved);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !changed) return;
    const note = draft.trim();
    const result = await patch({ running_joke: note }, note ? `${name}’s running-joke note saved.` : `${name}’s running-joke note cleared.`, true);
    error = result.ok ? '' : refusalWords(result.code ?? '', result.message, note, names);
  }
</script>

<div class="membersheet__section tease" data-fid="members-tease">
  <p class="membersheet__label">Reminder teasing</p>
  <div class="tease__switchrow">
    <span class="tease__text">
      <span class="tease__title" id="{uid}-t">OK to be teased</span>
      <span class="note" id="{uid}-d">{name} can change this on their portal Profile. Changes show in History.</span>
    </span>
    <button
      type="button"
      role="switch"
      class="switch"
      aria-checked={member.tease_ok}
      aria-labelledby="{uid}-t"
      aria-describedby="{uid}-d"
      aria-disabled={busy}
      data-fid="members-tease-switch"
      onclick={() => {
        if (!busy) void patch({ tease_ok: !member.tease_ok }, member.tease_ok ? `${name} is no longer OK to be teased.` : `${name} is OK to be teased.`);
      }}><span class="switch__knob" aria-hidden="true"></span></button
    >
  </div>
  <form class="field tease__note" data-fid="members-tease-note" onsubmit={save}>
    <span id="{uid}-n">Running-joke note</span>
    <div class="tease__inputrow">
      <input
        bind:value={draft}
        maxlength={NOTE_MAX}
        autocomplete="off"
        aria-labelledby="{uid}-n"
        aria-describedby="{uid}-h{error ? ` ${uid}-e` : ''}"
        aria-invalid={error ? 'true' : undefined}
        oninput={() => (error = '')}
      />
      <button class="btn" type="submit" disabled={busy || !changed}>Save</button>
    </div>
    <p class="tease__hint" id="{uid}-h">
      <span>Plain text: no names, IGNs, mentions or links. {name} sees it on their Profile. Sent only while they allow teasing.</span>
      <span class="tease__count">{[...draft.trim()].length} / {NOTE_MAX}</span>
    </p>
    {#if error}<p class="field__error" role="alert" id="{uid}-e" data-fid="members-tease-error">{error}</p>{/if}
  </form>
</div>

<style>
  .tease__switchrow {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .tease__text {
    display: flex;
    flex: 1 1 auto;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    line-height: 1.3;
  }

  .tease__title {
    color: var(--ink);
    font-size: var(--fs-body-sm);
    font-weight: 600;
  }

  .tease__inputrow {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .tease__inputrow input {
    flex: 1 1 auto;
    min-width: 0;
  }

  .tease__hint {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    margin: 0;
    color: var(--dim-text);
    font-size: var(--fs-mini);
    line-height: 1.35;
  }

  .tease__count {
    flex: none;
    font-family: var(--mono);
  }

  .tease__note .field__error {
    margin: 0;
  }
</style>
