<script lang="ts">
  import { tick } from 'svelte';
  import type { MemberPatch, MemberRow, Persona, PingLevel } from '@kanade/api-types';
  import { Avatar, enter, Icon, Modal, Select } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { send } from '../resource.svelte';
  import { directory } from '../names/directory.svelte';
  import Name from '../names/Name.svelte';
  import { memberAvatar } from '../shared/avatar';
  import { ANSWER_WORDS, type MemberWeek } from './runs';

  let {
    wide,
    member,
    runs,
    personas,
    onchange,
    onclose,
    leaving = false,
    onleft,
  }: {
    wide: boolean;
    member: MemberRow;
    runs: MemberWeek | null;
    personas: Persona[];
    onchange: (row: MemberRow) => void;
    onclose: () => void;
    /** Closed, playing its exit (MembersPage's Presence): inert, and the phone dialog closes. */
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
  } = $props();
  // A primitive key: an edited copy of the same member must not replay the enter.
  const memberId = $derived(member.id);

  const uid = $props.id();
  const LEVELS: { key: PingLevel; label: string; hint: string }[] = [
    { key: 'essential', label: 'Essential', hint: 'only where they have to act' },
    { key: 'all', label: 'All', hint: 'every post that names them' },
    { key: 'off', label: 'Off', hint: 'named, never notified' },
  ];
  const ACCESS = { staff: 'Staff — exempt from chatbot budgets', pilot: 'Chat pilot', none: 'No chatbot access' };

  let alias = $state('');
  let notice = $state<{ ok: boolean; message: string } | null>(null);
  let busy = $state(false);
  let seeded: string | null = null;

  $effect(() => {
    if (member && seeded !== member.id) {
      seeded = member.id;
      alias = '';
      notice = null;
    }
  });

  async function patch(change: MemberPatch, done: string) {
    if (!member) return;
    busy = true;
    const id = member.id;
    const result = await send((c) => c.patch<MemberRow>(`/api/admin/members/${encodeURIComponent(id)}`, change));
    busy = false;
    notice = result.ok ? { ok: true, message: done } : { ok: false, message: result.message };
    if (result.ok) onchange(result.value);
  }

  async function addAlias(event: SubmitEvent) {
    event.preventDefault();
    if (!member) return;
    busy = true;
    const id = member.id;
    const result = await send((c) => c.post<MemberRow>(`/api/admin/members/${encodeURIComponent(id)}/aliases`, { alias }));
    busy = false;
    if (result.ok) {
      notice = { ok: true, message: `Alias “${alias.trim().toLowerCase()}” added.` };
      alias = '';
      onchange(result.value);
    } else {
      notice = { ok: false, message: result.message };
    }
  }

  let aliasList = $state<HTMLElement>();
  let aliasInput = $state<HTMLInputElement>();

  // No confirmation: an alias is one word and re-adding it is one step.
  async function removeAlias(name: string) {
    if (!member) return;
    busy = true;
    const id = member.id;
    const index = member.aliases.indexOf(name);
    const result = await send((c) => c.delete<MemberRow>(`/api/admin/members/${encodeURIComponent(id)}/aliases/${encodeURIComponent(name)}`));
    busy = false;
    if (result.ok) {
      notice = { ok: true, message: `Alias “${name}” removed.` };
      onchange(result.value);
    } else {
      notice = { ok: false, message: result.message };
    }
    await tick();
    if (member?.id !== id) return;
    // The disabled button lost focus: land on the chip that took its place, else the add field.
    const buttons = aliasList?.querySelectorAll<HTMLButtonElement>('.membersheet__alias-remove') ?? [];
    const target = result.ok ? (buttons[index] ?? aliasInput) : buttons[index];
    target?.focus({ preventScroll: true });
  }
</script>

<svelte:window
  onkeydown={(event) => {
    if (wide && !leaving && event.key === 'Escape') {
      event.preventDefault();
      onclose();
    }
  }}
/>

<!-- The same editor is a non-modal side pane on wide screens and a full-screen
     dialog below the shared 900 px single-pane breakpoint. -->
{#snippet content()}
  <div class="membersheet__content">
    <dl class="membersheet__grid" data-fid="members-facts">
      {#if !wide}
        <dt>Discord account</dt>
        <dd class="membersheet__account"><Avatar class="membersheet__avatar membersheet__avatar--inline" src={memberAvatar(member.id)} name={member.name} /><span><Name kind="member" id={member.id} name={member.name} /> <span class="note">· select to copy the ID</span></span></dd>
      {/if}
      <dt>Server nickname</dt>
      <dd>{member.nickname ?? '—'}</dd>
      <dt>Runs this week</dt>
      <dd>
        <span class="membersheet__count mono">{member.runs_this_week}</span>
        {#if runs?.next}<span class="note">· next {runs.next.when} {runs.next.title}</span>{/if}
      </dd>
      <dt>Bossing role</dt>
      <dd>{member.bossing ? 'Yes — on the roster' : 'No — not on the roster'}</dd>
      <dt>Chatbot</dt>
      <dd>{ACCESS[member.access]}</dd>
    </dl>

    <div class="membersheet__section" data-fid="members-mentions">
      <p class="membersheet__label" id="{uid}-ping">@mentions</p>
      <div class="seg" role="group" aria-labelledby="{uid}-ping">
        {#each LEVELS as level (level.key)}
          <button
            type="button"
            class="seg__btn"
            aria-pressed={member.ping_level === level.key}
            disabled={busy}
            title={level.hint}
            onclick={() => member.ping_level !== level.key && void patch({ ping_level: level.key }, `Pings set to ${level.label.toLowerCase()}.`)}
            >{level.label}</button
          >
        {/each}
      </div>
      <p class="note">{LEVELS.find((l) => l.key === member.ping_level)?.hint}</p>
    </div>

    <div class="membersheet__section" data-fid="members-style">
      <div class="field">
        <span>Reply style</span>
        <Select
          label="Reply style"
          value={member.persona ?? ''}
          options={[
            { value: '', label: 'Default' },
            ...personas.filter((p) => p.key !== 'default').map((p) => ({ value: p.key, label: p.name })),
            ...(member.persona && !member.persona_available ? [{ value: member.persona, label: member.persona, sub: 'unavailable' }] : []),
          ]}
          noun="styles"
          disabled={busy}
          onchange={(key) => {
            void patch({ persona: key }, key ? `Reply style set to ${personas.find((p) => p.key === key)?.name ?? key}.` : 'Back to the default reply style.');
          }}
        />
      </div>
      {#if member.persona && !member.persona_available}
        <p class="status status--at_risk">“{member.persona}” is no longer offered; replies use the default.</p>
      {/if}
    </div>

    <div class="membersheet__section" data-fid="members-aliases">
      <p class="membersheet__label">Chat aliases</p>
      <div class="membersheet__aliases" bind:this={aliasList}>
        {#each member.aliases as name (name)}<span class="membersheet__alias"
            >{name}<button
              type="button"
              class="membersheet__alias-remove"
              aria-label="Remove alias {name} from {member.name}"
              title="Remove alias"
              disabled={busy}
              onclick={() => void removeAlias(name)}><Icon name="x" /></button
            ></span
          >{:else}<span class="id">none</span>{/each}
      </div>
      <form class="membersheet__alias-form" onsubmit={addAlias}>
        <input bind:this={aliasInput} bind:value={alias} placeholder="New alias" size="12" required aria-label="New alias for {member.name}" />
        <button class="btn" type="submit" disabled={busy}>Add</button>
      </form>
    </div>
    {#if runs}
      <section class="membersheet__section" data-fid="members-runs" aria-labelledby="{uid}-week">
        <h3 class="membersheet__label" id="{uid}-week">This week</h3>
        {#if runs.runs.length}
          <ul class="membersheet__runs">
            {#each runs.runs as run (run.id)}
              <li class="membersheet__run" data-fid="members-run" class:membersheet__run--past={!run.upcoming}>
                <span class="mono membersheet__when">{run.when}</span>
                <span class="membersheet__title">{run.title}</span>
                <span class="membersheet__answer membersheet__answer--{run.answer}">{ANSWER_WORDS[run.answer]}</span>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="note">Not on a run this week.</p>
        {/if}
      </section>
    {/if}
    <p class="membersheet__notice" class:field__error={notice && !notice.ok} role="status">{notice?.message ?? ''}</p>
  </div>
{/snippet}

{#if wide}
  <aside class="side-pane" class:is-leaving={leaving} inert={leaving} aria-label="Member details" data-fid="members-pane" onanimationend={onleft} {@attach enter(memberId)}>
    <header class="membersheet__head" data-fid="members-pane-head">
      <Avatar class="membersheet__avatar" src={memberAvatar(member.id)} name={member.name} />
      <div class="membersheet__who">
        <p class="cap">{member.bossing ? 'Member' : 'Chat access only'}</p>
        <h2>{directory.label('member', member.id, member.name)}</h2>
        <p class="membersheet__handle"><Name kind="member" id={member.id} name={member.name} /> · select to copy the ID</p>
      </div>
      <button class="btn btn--ghost membersheet__close" data-fid="members-close" type="button" aria-label="Close member details" onclick={onclose}><Icon name="x" /></button>
    </header>
    {@render content()}
  </aside>
{:else}
  <Modal open={!leaving} title={directory.label('member', member.id, member.name)} eyebrow={member.bossing ? 'Member' : 'Chat access only'} narrow className="membersheet" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
