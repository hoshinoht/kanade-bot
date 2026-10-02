<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import type { BossRow, FixedRow } from '@kanade/api-types';
  import { BossTag, Icon, Modal, Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/fixed.scss';
  import Name from '../names/Name.svelte';
  import { directory } from '../names/directory.svelte';
  import { Resource, send } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import FixedEditor from './FixedEditor.svelte';
  import { FixedSnapshot } from './snapshot.svelte';

  let {
    store,
    toaster,
    openId = '',
    onopened = () => {},
  }: { store: AdminWeek; toaster: Toaster; openId?: string; onopened?: () => void } = $props();

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

  /**
   * The whole row card answers a pointer (B_Fixed) without a positioned
   * overlay -- WebKit (bug 240961) lets a `::after` on a row escape to the
   * frame. Clicks that land on another control (a name's copy button) stay
   * theirs; the keyboard keeps using the row's button.
   */
  function forwardRowClicks(body: HTMLElement) {
    const onclick = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (!target || target.closest('button, a, input, select, textarea, label')) return;
      target.closest('tr')?.querySelector<HTMLButtonElement>('.fixed-list__open')?.click();
    };
    body.addEventListener('click', onclick);
    return () => body.removeEventListener('click', onclick);
  }

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

  /** A deep link (`/fixed?open=<id>`) opens that timing's editor once its row is on screen. */
  $effect(() => {
    if (!openId || !fixed.rows) return;
    const row = fixed.rows.find((candidate) => candidate.id === openId);
    const opener = document.querySelector<HTMLButtonElement>(`.fixed-list__open[data-fixed="${CSS.escape(openId)}"]`);
    if (row && opener) {
      opener.scrollIntoView({ block: 'nearest' });
      open(row, opener);
    }
    onopened();
  });

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
  <h1>{#if fixed.rows}<span class="pageline__num">{fixed.rows.length}</span> weekly timing{fixed.rows.length === 1 ? '' : 's'}{:else}Weekly timings{/if}</h1>
  <p class="pageline__context">the baseline, materialised into runs for this week and next</p>
  {#snippet side()}
    <div class="page-head__side">
      <button class="btn btn--primary fixed-add" type="button" data-fid="fixed-add" data-fixed-add bind:this={addTrigger} onclick={(event) => open(null, event.currentTarget)}><Icon name="plus" />Add a weekly timing</button>
    </div>
  {/snippet}
</PageLine>

<section data-fid="window" class="card fixed-window window-fill" aria-labelledby="fixed-title">
  <div class="card__head fixed-window__head" data-fid="window-bar">
    <h2 class="card__title" id="fixed-title">Weekly timings</h2>
    <div class="fixed-window__search" data-fid="window-search" role="search">
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
    <div class="fixed-list" data-fid="fixed-list">
      <div class="fixed-list__table">
      <!-- B_Fixed lays the rows out as grid cards; the explicit roles keep the
           table semantics that a non-table display would drop. -->
      <!-- svelte-ignore a11y_no_redundant_roles -->
      <table role="table" aria-labelledby="fixed-caption">
        <caption class="vh" id="fixed-caption">Weekly timings, by weekday</caption>
        <!-- svelte-ignore a11y_no_redundant_roles -->
        <thead role="rowgroup">
          <!-- svelte-ignore a11y_no_redundant_roles -->
          <tr role="row" data-fid="fixed-head">
            <th role="columnheader" scope="col">When</th>
            <th role="columnheader" scope="col">Bosses</th>
            <th role="columnheader" scope="col">Party</th>
          </tr>
        </thead>
        <!-- svelte-ignore a11y_no_redundant_roles -->
        <tbody role="rowgroup" {@attach forwardRowClicks}>
          {#each rows as row (row.id)}
            {@const changed = row.runs.filter((r) => r.amended).length}
            <!-- svelte-ignore a11y_no_redundant_roles -->
            <tr role="row" data-fid="fixed-row" class:fixed-list__row--active={editorOpen && editing?.id === row.id}>
               <!-- v4 order: the time leads; the bosses stay the row's header. The
                    button is the row's one control; a pointer anywhere else on
                    the row is forwarded to it (`forwardRowClicks`); the party's
                    names are plain text here (the editor copies ids). -->
               <td role="cell">
                <button
                  class="fixed-list__open"
                  type="button"
                  aria-current={editorOpen && editing?.id === row.id ? 'true' : undefined}
                  aria-label="Edit {title(row)}"
                  data-fixed={row.id}
                  onclick={(event) => open(row, event.currentTarget)}
                >
                  <span class="fixed-list__when"><span class="fixed-list__day">{row.weekday_name}</span> <span class="mono fixed-list__time">{row.time}</span></span>
                  {#if editorOpen && editing?.id === row.id}<span class="cap">open</span>{/if}
                </button>
                <!-- The machine id is for search and the editor's head, not the scan (B_Fixed). -->
                <span class="vh">#{row.short_id}</span>
               </td>
              <th role="rowheader" scope="row" class="fixed-list__bosses">
                <!-- B_Fixed: the bosses, then the row's flags inline in quiet mono words. -->
                <div class="fixed-list__bossline">
                  <ul class="bosslist">{#each row.bosses as boss (boss.token)}<li><BossTag {boss} portrait /></li>{/each}</ul>
                  {#if changed}<span class="status status--planned">{changed} run{changed === 1 ? '' : 's'} amended</span>{/if}
                  {#if !row.channel_watched}<span class="status status--at_risk">not watched</span>{/if}
                </div>
                {#if row.note}<span class="note">{row.note}</span>{/if}
              </th>
              <td role="cell" class="fixed-list__party" title={row.participants.map((person) => directory.label('member', person.id, person.name)).join(' · ')}><span class="chips">{#each row.participants as person (person.id)}<span class="chip"><Name kind="member" id={person.id} name={person.name} plain /></span>{/each}</span></td>
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
