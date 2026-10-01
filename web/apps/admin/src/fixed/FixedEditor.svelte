<!--
  Add or edit a weekly timing (v4 fixed.html / fixed_rows.html editor). Saving
  an edit whose timing has amended runs this week or next asks, per run,
  whether it follows the new timing or keeps its own change (v5).
-->
<script lang="ts">
  import type { Boss, BossRow, Channel, FixedRequest, FixedRow, MemberRow, ValidateResult } from '@kanade/api-types';
  import { BossTag, Modal, SidePane, dayLabel } from '@kanade/ui';
  import '@kanade/ui/styles/fixed.scss';
  import BossGrid from '../bosses/BossGrid.svelte';
  import { send } from '../resource.svelte';
  import { memberLabel } from '../names/directory.svelte';
  import type { Week } from '@kanade/api-types';

  let {
    open = $bindable(false),
    wide,
    modalOpen,
    row,
    bosses,
    channels,
    members,
    week,
    version,
    onsaved,
    onstale,
    onclose,
    onretire,
  }: {
    open: boolean;
    wide: boolean;
    modalOpen: boolean;
    /** null = add a new timing. */
    row: FixedRow | null;
    bosses: BossRow[];
    channels: Channel[];
    members: MemberRow[];
    week: Week | null;
    /** Week version `row` was read at. */
    version: number | null;
    onsaved: (row: FixedRow, message: string) => void;
    /** The timing changed underneath (409): the page re-reads it. */
    onstale: () => void;
    onclose: () => void;
    onretire: (row: FixedRow) => void;
  } = $props();

  const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
  const uid = $props.id();

  let weekday = $state(0);
  let time = $state('');
  let channel = $state('');
  let note = $state('');
  let party = $state<string[]>([]);
  let selected = $state<string[]>([]);
  let typed = $state('');
  let check = $state<{ bosses: Boss[] } | { error: string } | null>(null);
  let error = $state('');
  let busy = $state(false);
  let step = $state<'edit' | 'choose'>('edit');
  let decisions = $state<Record<string, 'update' | 'keep'>>({});
  let seeded: string | null = null;
  /** Pinned when the form opens, so a re-read behind it cannot turn a stale save into an overwrite. */
  let formVersion: number | null = null;

  $effect(() => {
    const key = row?.id ?? 'new';
    if (open && seeded !== key) {
      seeded = key;
      formVersion = version;
      weekday = row?.weekday ?? 0;
      time = row?.time ?? '';
      channel = row?.channel_id ?? channels[0]?.id ?? '';
      note = row?.note ?? '';
      party = row?.participants.map((p) => p.id) ?? [];
      selected = row?.bosses.map((b) => b.token) ?? [];
      typed = '';
      check = null;
      error = '';
      step = 'edit';
      decisions = {};
    }
    if (!open) seeded = null;
  });

  // v4 bosscheck: the typed field is validated as you type (debounced).
  $effect(() => {
    const text = typed.trim();
    if (!text) {
      check = null;
      return;
    }
    const timer = setTimeout(async () => {
      const result = await send((c) => c.post<ValidateResult>('/api/admin/validate/bosses', { text }));
      if (typed.trim() === text) check = result.ok ? { bosses: result.value.bosses } : { error: result.message };
    }, 400);
    return () => clearTimeout(timer);
  });

  const amended = $derived(row?.runs.filter((r) => r.amended) ?? []);
  const roster = $derived(members.filter((m) => m.bossing));

  function request(): FixedRequest {
    return {
      weekday: Number(weekday),
      time: time.trim(),
      bosses: [...selected, typed.trim()].filter(Boolean).join(' '),
      participants: party,
      channel_id: channel,
      note: note.trim() || null,
      decisions,
    };
  }

  async function save() {
    busy = true;
    error = '';
    const body = request();
    const result = row
      ? await send((c) => c.patch<FixedRow>(`/api/admin/fixed/${encodeURIComponent(row.id)}`, { ...body, version: formVersion ?? undefined }))
      : await send((c) => c.post<FixedRow>('/api/admin/fixed', body));
    busy = false;
    if (!result.ok) {
      // Only `stale` means the form is out of date; `busy` and the rest leave it valid to retry as is.
      const stale = result.code === 'stale';
      error = stale ? `${result.message} Close and reopen this timing to edit what is saved now.` : result.message;
      step = 'edit';
      if (stale) onstale();
      return;
    }
    const title = `${result.value.weekday_name} ${result.value.time} — ${result.value.bosses.map((b) => b.token).join(' + ')}`;
    open = false;
    onsaved(result.value, row ? `Saved ${title}.` : `Added ${title}; its runs are on the board.`);
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (step === 'edit' && amended.length > 0) {
      decisions = Object.fromEntries(amended.map((r) => [r.run_id, decisions[r.run_id] ?? 'keep']));
      step = 'choose';
      return;
    }
    void save();
  }

  function when(run: FixedRow['runs'][number]): string {
    const day = week && run.week === 'this' ? dayLabel(week, run.day) : `${run.week} week, day ${run.day + 1}`;
    return `${day} ${run.time ?? 'own time'}`;
  }
</script>

<svelte:window onkeydown={(event) => {
  if (wide && !modalOpen && event.key === 'Escape') {
    event.preventDefault();
    onclose();
  }
}} />

{#snippet formBody()}
  <form id="{uid}-form" class="fixedsheet__form" onsubmit={submit} novalidate>
    <div class="fixedsheet__content">
    {#if step === 'edit'}
      <p class="eyebrow">Bosses — tap the difficulties this party runs</p>
      <div class="fixedsheet__bosses"><BossGrid rows={bosses} bind:selected /></div>
      <label class="field fixedsheet__typed">
          <span>…or type them</span>
          <input bind:value={typed} placeholder="hstar, hfa" aria-describedby="{uid}-check" />
      </label>
      <span class="boss-check" id="{uid}-check" role="status">
        {#if check && 'error' in check}<span class="status status--at_risk">{check.error}</span>
        {:else if check}{#each check.bosses as boss (boss.token)}<BossTag {boss} />{/each}{/if}
      </span>
      <div class="fixedsheet__fields">
        <label class="field">
          <span>Day</span>
          <select bind:value={weekday}>
            {#each WEEKDAYS as name, index (name)}<option value={index}>{name}</option>{/each}
          </select>
        </label>
        <label class="field"><span>Time</span><input bind:value={time} placeholder="21:30" size="6" class="mono" /></label>
        <label class="field">
          <span>Home channel</span>
          <select bind:value={channel}>
            {#each channels as c (c.id)}<option value={c.id}>{c.name}</option>{/each}
          </select>
        </label>
      </div>
      <label class="field fixedsheet__note"><span>Note</span><input bind:value={note} /></label>
      <fieldset class="field">
        <legend class="label">Party</legend>
        <div class="run__people">
          {#each roster as member (member.id)}
            <label class="chip">
              <input type="checkbox" value={member.id} bind:group={party} />
              {memberLabel(roster, member.id)}
            </label>
          {/each}
        </div>
      </fieldset>
    {:else}
      <p>
        {amended.length === 1 ? 'One run from this timing was' : `${amended.length} runs from this timing were`} changed
        for their week. Choose what each does with the new timing:
      </p>
      {#each amended as run (run.run_id)}
        <fieldset class="field choice">
          <legend class="label">#{run.short_id} · {when(run)}</legend>
          <label class="choice__opt">
            <input type="radio" name="{uid}-{run.run_id}" value="update" bind:group={decisions[run.run_id]} />
            Update to the new timing
          </label>
          <label class="choice__opt">
            <input type="radio" name="{uid}-{run.run_id}" value="keep" bind:group={decisions[run.run_id]} />
            Keep this week's change
          </label>
        </fieldset>
      {/each}
    {/if}
    <p class="field__error" role="alert">{error}</p>
    </div>
    {#if wide}
      <footer class="fixedsheet__foot">
        {#if row && step === 'edit'}<button class="btn btn--danger" type="button" onclick={() => onretire(row)}>Retire…</button>{/if}
        {#if step === 'choose'}
          <button class="btn" type="button" onclick={() => (step = 'edit')}>Back</button>
        {:else}
          <button class="btn" type="button" onclick={onclose}>Cancel</button>
        {/if}
        <button class="btn btn--primary" type="submit" form="{uid}-form" disabled={busy}>
          {row ? (step === 'edit' && amended.length ? 'Save…' : 'Save changes') : 'Add timing'}
        </button>
      </footer>
    {/if}
  </form>
{/snippet}

{#if wide}
  <SidePane label="Weekly timing details" className="side-pane--fixed">
    <header class="fixedsheet__head">
      <div>
        <p class="cap">{row ? `#${row.short_id} · edit` : 'Baseline · new'}</p>
        <h2>{row ? `${row.weekday_name} ${row.time} — ${row.bosses.map((b) => b.token).join(' + ')}` : 'New weekly timing'}</h2>
      </div>
      <button class="btn btn--ghost fixedsheet__close" type="button" aria-label="Close weekly timing details" onclick={onclose}>×</button>
    </header>
    {@render formBody()}
  </SidePane>
{:else}
  <Modal bind:open title={row ? `${row.weekday_name} ${row.time} — ${row.bosses.map((b) => b.token).join(' + ')}` : 'Add a weekly timing'} eyebrow={row ? `#${row.short_id} · ${row.channel_name}` : 'Baseline'} narrow className="fixedsheet" onclose={onclose}>
    {@render formBody()}
    {#snippet footer(close)}
    {#if step === 'choose'}
      <button class="btn" type="button" onclick={() => (step = 'edit')}>Back</button>
    {:else}
      <button class="btn" type="button" onclick={close}>Cancel</button>
    {/if}
    <button class="btn btn--primary" type="submit" form="{uid}-form" disabled={busy}>
      {row ? (step === 'edit' && amended.length ? 'Save…' : 'Save changes') : 'Add timing'}
    </button>
  {/snippet}
  </Modal>
{/if}
