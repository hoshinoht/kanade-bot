<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import type { BossRow, FixedRow } from '@kanade/api-types';
  import { BossTag, Modal, Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/fixed.scss';
  import Name from '../names/Name.svelte';
  import { Resource, send } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import FixedEditor from './FixedEditor.svelte';
  import { FixedSnapshot } from './snapshot.svelte';

  let { store, toaster }: { store: AdminWeek; toaster: Toaster } = $props();

  /** Rows and the week version they were read at, published together; edits send that version. */
  const fixed = new FixedSnapshot();
  const bosses = new Resource<BossRow[]>('/api/admin/bosses');
  $effect(() => {
    void load();
    void bosses.load();
  });

  function load() {
    return fixed.load();
  }

  let query = $state('');
  let editing = $state<FixedRow | null>(null);
  let editorOpen = $state(false);
  let wide = $state(false);
  let restoreElement: HTMLButtonElement | null = null;
  let addTrigger: HTMLButtonElement;
  let retireOpen = $state(false);
  let retiring = $state<FixedRow | null>(null);
  let retireError = $state('');
  let retireFocus = $state(false);

  const title = (row: FixedRow) => `${row.weekday_name} ${row.time} — ${row.bosses.map((b) => b.token).join(' + ')}`;
  const q = $derived(query.trim().toLowerCase());
  const rows = $derived(
    (fixed.rows ?? []).filter(
      (row) =>
        !q ||
        [row.weekday_name, row.time, row.channel_name, row.owner, row.note ?? '', ...row.participants.map((p) => p.name), ...row.bosses.flatMap((b) => [b.token, b.name])].some(
          (t) => t.toLowerCase().includes(q),
        ),
    ),
  );

  function open(row: FixedRow | null, opener: HTMLButtonElement) {
    editing = row;
    restoreElement = opener;
    editorOpen = true;
  }

  $effect(() => {
    const media = window.matchMedia('(min-width: 840px)');
    const update = () => (wide = media.matches);
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  });

  function closeEditor(restoreFocus = true) {
    editorOpen = false;
    editing = null;
    if (!restoreFocus) return;
    requestAnimationFrame(() => {
      (restoreElement?.isConnected ? restoreElement : addTrigger)?.focus();
    });
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
    const n = result.value.cancelled;
    toaster.show({ message: `Retired ${title(row)}; ${n} upcoming run${n === 1 ? '' : 's'} cancelled.`, tone: 'ok' });
    await load();
    void store.refresh();
    closeEditor(false);
    retireFocus = true;
    retireOpen = false;
  }
</script>

<PageLine title={fixed.rows ? 'Fixed' : ''}>
  <h1>{fixed.rows ? `${fixed.rows.length} weekly timing${fixed.rows.length === 1 ? '' : 's'}` : 'Weekly timings'}</h1>
  <p class="pageline__context">the baseline, materialised into runs for this week and next</p>
  {#snippet side()}
    <div class="page-head__side">
      <button class="btn btn--primary" type="button" data-fixed-add bind:this={addTrigger} onclick={(event) => open(null, event.currentTarget)}>Add a weekly timing</button>
    </div>
  {/snippet}
</PageLine>

<section class="card fixed-window window-fill" aria-labelledby="fixed-title">
  <div class="card__head fixed-window__head">
    <span class="fixed-window__dots" aria-hidden="true"><i></i><i></i><i></i></span>
    <h2 class="card__title" id="fixed-title">Weekly timings</h2>
    <div class="fixed-window__search" role="search">
      <label class="vh" for="fixed-search">Search weekly timings</label>
      <input id="fixed-search" type="search" bind:value={query} placeholder="boss, day, party, channel…" autocomplete="off" spellcheck="false" />
    </div>
  </div>
  <div class="fixed-window__body">
  {#if fixed.error}
    <p class="flash flash--error" role="alert">{fixed.error}</p>
  {:else if fixed.rows && rows.length === 0}
    <div class="empty">
      {#if q}<strong>Nothing matches “{query}”.</strong>The search reads the bosses, the day and time, the party and the home channel.
      {:else}<strong>No baseline yet.</strong>Add one with the button above, or run <code>/fixed add</code> inside a party channel.{/if}
    </div>
  {:else if fixed.rows}
    <div class="fixed-list">
      <div class="fixed-list__table">
      <table>
        <caption class="vh">Weekly timings, by weekday</caption>
        <thead>
          <tr>
            <th scope="col">When</th>
            <th scope="col">Bosses</th>
            <th scope="col">Party</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.id)}
            {@const changed = row.runs.filter((r) => r.amended).length}
            <tr class:fixed-list__row--active={editorOpen && editing?.id === row.id}>
               <!-- v4 order: the time leads; the bosses stay the row's header. -->
               <td>
                <button
                  class="fixed-list__open"
                  class:fixed-list__open--active={editorOpen && editing?.id === row.id}
                  type="button"
                  aria-current={editorOpen && editing?.id === row.id ? 'true' : undefined}
                  aria-label="Edit {title(row)}"
                  data-fixed={row.id}
                  onclick={(event) => open(row, event.currentTarget)}
                >
                  <span class="fixed-list__when"><strong>{row.weekday_name}</strong> <span class="mono">{row.time}</span></span>
                  {#if editorOpen && editing?.id === row.id}<span class="cap">open</span>{/if}
                </button>
                <div class="id">#{row.short_id}</div>
                {#if changed}<div class="status status--planned">{changed} run{changed === 1 ? '' : 's'} amended</div>{/if}
               </td>
              <th scope="row" class="fixed-list__bosses">
                <ul class="bosslist">{#each row.bosses as boss (boss.token)}<li><BossTag {boss} portrait /></li>{/each}</ul>
                {#if row.note}<span class="note">{row.note}</span>{/if}
                {#if !row.channel_watched}<div class="status status--at_risk">not watched</div>{/if}
              </th>
              <td class="fixed-list__party"><span class="chips">{#each row.participants as person (person.id)}<span class="chip"><Name kind="member" id={person.id} name={person.name} /></span>{/each}</span></td>
             </tr>
          {/each}
        </tbody>
      </table>
      </div>
    </div>
  {:else}
    <p class="note" aria-busy="true">Loading the weekly timings…</p>
  {/if}
    {#if editorOpen}
      <FixedEditor
        bind:open={editorOpen}
        {wide}
        modalOpen={retireOpen}
        row={editing}
        bosses={bosses.data ?? []}
        channels={store.channels}
        members={store.members}
        week={store.week}
        version={fixed.version}
        onsaved={saved}
        onstale={stale}
        onclose={closeEditor}
        onretire={(row) => {
          retiring = row;
          retireError = '';
          retireOpen = true;
        }}
      />
    {/if}
  </div>
</section>


<!-- Retiring cancels runs people are counting on: named consequence, explicit confirm (RECOVER-3). -->
<Modal bind:open={retireOpen} title={retiring ? `Retire ${title(retiring)}?` : 'Retire'} eyebrow="Baseline" narrow returnFocus={() => {
  if (!retireFocus) return null;
  retireFocus = false;
  return addTrigger;
}}>
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
