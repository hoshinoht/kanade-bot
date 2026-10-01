<script lang="ts">
  import type { MemberPatch, MemberRow, Persona, PingLevel } from '@kanade/api-types';
  import { Modal, SidePane } from '@kanade/ui';
  import { send } from '../resource.svelte';
  import { directory } from '../names/directory.svelte';
  import Name from '../names/Name.svelte';

  let {
    wide,
    member,
    personas,
    onchange,
    onclose,
  }: { wide: boolean; member: MemberRow; personas: Persona[]; onchange: (row: MemberRow) => void; onclose: () => void } = $props();

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
</script>

<svelte:window
  onkeydown={(event) => {
    if (wide && event.key === 'Escape') {
      event.preventDefault();
      onclose();
    }
  }}
/>

<!-- The same editor is a non-modal side pane on wide screens and a full-screen
     dialog below the approved 840px breakpoint. -->
{#snippet content()}
  <div class="membersheet__content">
    <dl class="membersheet__grid">
      <dt>Discord account</dt>
      <dd><Name kind="member" id={member.id} name={member.name} /> <span class="note">(select to copy the ID)</span></dd>
      <dt>Server nickname</dt>
      <dd>{member.nickname ?? '—'}</dd>
      <dt>Runs this week</dt>
      <dd class="mono">{member.runs_this_week}</dd>
      <dt>Bossing role</dt>
      <dd>{member.bossing ? 'Yes — on the roster' : 'No — not on the roster'}</dd>
      <dt>Chatbot</dt>
      <dd>{ACCESS[member.access]}</dd>
    </dl>

    <div class="membersheet__section">
      <p class="membersheet__label" id="{uid}-ping">@mentions</p>
      <div class="seg seg--answer" role="group" aria-labelledby="{uid}-ping">
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

    <div class="membersheet__section">
      <label class="field">
        <span>Reply style</span>
        <select
          value={member.persona ?? ''}
          disabled={busy}
          onchange={(event) => {
            const key = event.currentTarget.value;
            void patch({ persona: key }, key ? `Reply style set to ${personas.find((p) => p.key === key)?.name ?? key}.` : 'Back to the default reply style.');
          }}
        >
          <option value="">Default</option>
          {#each personas.filter((p) => p.key !== 'default') as persona (persona.key)}
            <option value={persona.key}>{persona.name}</option>
          {/each}
          {#if member.persona && !member.persona_available}<option value={member.persona}>{member.persona} (unavailable)</option>{/if}
        </select>
      </label>
      {#if member.persona && !member.persona_available}
        <p class="status status--at_risk">“{member.persona}” is no longer offered; replies use the default.</p>
      {/if}
    </div>

    <div class="membersheet__section">
      <p class="membersheet__label">Chat aliases</p>
      <div class="membersheet__aliases">
        {#each member.aliases as name (name)}<span class="chip chip--mono">{name}</span>{:else}<span class="id">none</span>{/each}
      </div>
      <form class="membersheet__alias-form" onsubmit={addAlias}>
        <input bind:value={alias} placeholder="New alias" size="12" required aria-label="New alias for {member.name}" />
        <button class="btn" type="submit" disabled={busy}>Add</button>
      </form>
    </div>
    <p class="membersheet__notice" class:field__error={notice && !notice.ok} role="status">{notice?.message ?? ''}</p>
  </div>
{/snippet}

{#if wide}
  <SidePane label="Member details">
    <header class="membersheet__head">
      <span class="membersheet__avatar" aria-hidden="true">{member.name.slice(0, 1)}</span>
      <div>
        <p class="cap">{member.bossing ? 'Member' : 'Chat access only'}</p>
        <h2>{directory.label('member', member.id, member.name)}</h2>
        <Name kind="member" id={member.id} name={member.name} />
      </div>
      <button class="btn btn--ghost membersheet__close" type="button" aria-label="Close member details" onclick={onclose}>×</button>
    </header>
    {@render content()}
  </SidePane>
{:else}
  <Modal open title={directory.label('member', member.id, member.name)} eyebrow={member.bossing ? 'Member' : 'Chat access only'} narrow className="membersheet" onclose={onclose}>
    {@render content()}
    {#snippet footer(close)}
      <button class="btn" type="button" onclick={close}>Close</button>
    {/snippet}
  </Modal>
{/if}
