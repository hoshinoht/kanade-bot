<script lang="ts">
  import type { BossRow, FixedRow, Week } from '@kanade/api-types';
  import { BossTag, Modal, Toaster } from '@kanade/ui';
  import PaneWindow from '../pages/PaneWindow.svelte';
  import { Resource, send } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import FixedEditor from './FixedEditor.svelte';

  let { store, toaster }: { store: AdminWeek; toaster: Toaster } = $props();

  const fixed = new Resource<FixedRow[]>('/api/admin/fixed');
  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  /** Week version the rows were read at; edits send it (409 if a timing moved since). */
  let loadedAt = $state<number | null>(null);
  $effect(() => {
    void load();
    void bosses.load();
  });

  // Head first: an older version can only cost a needless 409, a newer one could hide someone's edit.
  async function load() {
    const head = await send((c) => c.get<Week>('/api/admin/week'));
    loadedAt = head.ok ? head.value.version : null;
    await fixed.load();
  }

  let query = $state('');
  let editorOpen = $state(false);
  let editing = $state<FixedRow | null>(null);
  let retireOpen = $state(false);
  let retiring = $state<FixedRow | null>(null);
  let retireError = $state('');

  const title = (row: FixedRow) => `${row.weekday_name} ${row.time} — ${row.bosses.map((b) => b.token).join(' + ')}`;
  const q = $derived(query.trim().toLowerCase());
  const rows = $derived(
    (fixed.data ?? []).filter(
      (row) =>
        !q ||
        [row.weekday_name, row.time, row.channel_name, row.owner, row.note ?? '', ...row.participants.map((p) => p.name), ...row.bosses.flatMap((b) => [b.token, b.name])].some(
          (t) => t.toLowerCase().includes(q),
        ),
    ),
  );

  function open(row: FixedRow | null) {
    editing = row;
    editorOpen = true;
  }

  async function saved(_row: FixedRow, message: string) {
    toaster.show({ message, tone: 'ok' });
    await load();
    void store.refresh();
  }

  /** Like a stale run edit: re-read so reopening shows what is saved now. */
  function stale() {
    void load();
    void store.refresh();
  }

  async function retire() {
    if (!retiring) return;
    const row = retiring;
    const result = await send((c) => c.delete<{ cancelled: number }>(`/api/admin/fixed/${encodeURIComponent(row.id)}`));
    if (!result.ok) {
      retireError = result.message;
      return;
    }
    retireOpen = false;
    const n = result.value.cancelled;
    toaster.show({ message: `Retired ${title(row)}; ${n} upcoming run${n === 1 ? '' : 's'} cancelled.`, tone: 'ok' });
    await load();
    void store.refresh();
  }
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">Baseline</p>
    <h1>{fixed.data ? `${fixed.data.length} weekly timing${fixed.data.length === 1 ? '' : 's'}` : 'Weekly timings'}</h1>
    <p class="note">Materialised into runs for this week and next.</p>
  </div>
  <div class="page-head__side">
    <button class="btn btn--primary" type="button" onclick={() => open(null)}>Add a weekly timing</button>
  </div>
</div>

<PaneWindow title="Weekly timings" bind:query searchLabel="Search weekly timings" placeholder="boss, day, party, channel…">
  {#if fixed.error}
    <p class="flash flash--error" role="alert">{fixed.error}</p>
  {:else if fixed.data && rows.length === 0}
    <div class="empty">
      {#if q}<strong>Nothing matches “{query}”.</strong>The search reads the bosses, the day and time, the party and the home channel.
      {:else}<strong>No baseline yet.</strong>Add one with the button above, or run <code>/fixed add</code> inside a party channel.{/if}
    </div>
  {:else if fixed.data}
    <div class="table-wrap">
      <table>
        <caption class="vh">Weekly timings, by weekday</caption>
        <thead>
          <tr>
            <th scope="col">When</th>
            <th scope="col">Bosses</th>
            <th scope="col">Party</th>
            <th scope="col">Home channel</th>
            <th scope="col">Owner</th>
            <th scope="col"><span class="vh">Actions</span></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.id)}
            {@const changed = row.runs.filter((r) => r.amended).length}
            <tr>
              <!-- v4 order: the time leads; the bosses stay the row's header. -->
              <td>
                <strong>{row.weekday_name}</strong> <span class="mono">{row.time}</span>
                <div class="id">#{row.short_id}</div>
                {#if changed}<div class="status status--planned">{changed} run{changed === 1 ? '' : 's'} amended</div>{/if}
              </td>
              <th scope="row">
                <ul class="bosslist">{#each row.bosses as boss (boss.token)}<li><BossTag {boss} portrait /></li>{/each}</ul>
                {#if row.note}<span class="note">{row.note}</span>{/if}
              </th>
              <td><span class="chips">{#each row.participants as person (person.id)}<span class="chip">{person.name}</span>{/each}</span></td>
              <td>
                {row.channel_name}
                {#if !row.channel_watched}<div class="status status--at_risk">not watched</div>{/if}
              </td>
              <td>{row.owner}</td>
              <td>
                <div class="rowbtns">
                  <button class="btn" type="button" onclick={() => open(row)} aria-label="Edit {title(row)}">Edit</button>
                  <button
                    class="btn btn--danger"
                    type="button"
                    aria-label="Retire {title(row)}"
                    onclick={() => {
                      retiring = row;
                      retireError = '';
                      retireOpen = true;
                    }}>Retire</button
                  >
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <p class="note" aria-busy="true">Loading the weekly timings…</p>
  {/if}
</PaneWindow>

<FixedEditor
  bind:open={editorOpen}
  row={editing}
  bosses={bosses.data ?? []}
  channels={store.channels}
  members={store.members}
  week={store.week}
  version={loadedAt}
  onsaved={saved}
  onstale={stale}
/>

<!-- Retiring cancels runs people are counting on: named consequence, explicit confirm (RECOVER-3). -->
<Modal bind:open={retireOpen} title={retiring ? `Retire ${title(retiring)}?` : 'Retire'} eyebrow="Baseline" narrow>
  {#if retiring}
    {@const live = retiring.runs.length}
    <p>
      It stops being materialised, and its {live} upcoming run{live === 1 ? ' is' : 's are'} cancelled and announced in
      {retiring.channel_name}. Past runs and the audit trail stay.
    </p>
    <p class="field__error" role="alert">{retireError}</p>
  {/if}
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Keep it</button>
    <button class="btn btn--primary" type="button" onclick={() => void retire()}>Retire timing</button>
  {/snippet}
</Modal>
