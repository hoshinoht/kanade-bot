<!--
  Add or edit a weekly timing (v4 fixed.html / fixed_rows.html editor). Saving
  an edit whose timing has amended runs this week or next asks, per run,
  whether it follows the new timing or keeps its own change (v5).
-->
<script lang="ts">
  import type { Boss, BossRow, Channel, FixedRequest, FixedRow, MemberRow, ValidateResult } from '@kanade/api-types';
  import { BossTag, DayStrip, Modal, Select, TimeStepper, dayLabel, enter } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import '@kanade/ui/styles/fixed.scss';
  import '@kanade/ui/styles/move-picker.scss';
  import { tick } from 'svelte';
  import { BossGrid } from '@kanade/ui';
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
    leaving = false,
    onleft,
    timeStep = 30,
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
    /** Closed, playing its exit (FixedPage's Presence): inert, and the phone dialog closes. */
    leaving?: boolean;
    onleft?: (event: AnimationEvent) => void;
    /** The time stepper's step: Config → Run lengths default minutes, as the Move picker. */
    timeStep?: number;
  } = $props();
  // A primitive key: the pane's content enters again only for another timing (or a new one).
  const rowKey = $derived(row?.id ?? 'new');

  const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
  const uid = $props.id();
  // The weekday strip runs in boss-week order from the reset day (P_MoveStates "Reuse").
  const resetDay = $derived(Math.max(0, WEEKDAYS.findIndex((d) => d.startsWith(week?.days[0]?.dow ?? 'Thu'))));
  const weekdayStrip = $derived(
    WEEKDAYS.map((_, i) => (resetDay + i) % 7).map((value) => ({ value, dow: WEEKDAYS[value]!.slice(0, 3), label: WEEKDAYS[value]! })),
  );

  let weekday = $state(0);
  let time = $state('');
  // The stepper reads a typed "21:30"; anything else stays as typed for the server to judge.
  const timeMinutes = $derived.by(() => {
    const hit = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
    if (!hit) return null;
    const [h, m] = [Number(hit[1]), Number(hit[2])];
    return h < 24 && m < 60 ? h * 60 + m : null;
  });
  const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  let channel = $state('');
  let note = $state('');
  let party = $state<string[]>([]);
  /** A pinned owner, or '' = the default: whoever is first in the party (user decision 2026-10-09). */
  let pickedOwner = $state('');
  let selected = $state<string[]>([]);
  // A timing that already has bosses shows only theirs until asked for the rest.
  let allBosses = $state(false);
  let ownKeys = $state<string[]>([]);
  const shownBosses = $derived(allBosses ? bosses : bosses.filter((b) => ownKeys.includes(b.key)));
  // B_Fixed: the party shows its picked members and the first few others, then "+n".
  const PARTY_PREVIEW = 9;
  let allParty = $state(false);
  /** The party when the form opened: the preview stays put while chips are toggled. */
  let openParty = $state<string[]>([]);
  let partyChips = $state<HTMLElement>();
  let ownerSelect = $state<{ focus(): void }>();
  let typed = $state('');
  let check = $state<{ bosses: Boss[] } | { error: string } | null>(null);
  let error = $state('');
  /** A 422 about the owner reads out under the Day / Time / Owner line. */
  let ownerError = $state('');
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
      openParty = [...party];
      pickedOwner = row?.owner_pinned ? row.owner_id : '';
      ownerError = '';
      allParty = false;
      selected = row?.bosses.map((b) => b.token) ?? [];
      allBosses = !row;
      ownKeys = row?.bosses.map((b) => b.key) ?? [];
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
  /** Default first (it follows the party), then the roster, plus a pinned owner who has since left it. */
  const ownerOptions = $derived.by(() => {
    const first = party[0] ? memberLabel(roster, party[0]) : '';
    const options = [{ id: '', label: first ? `Default: first in party (${first})` : 'Default: first in party' }, ...roster.map((m) => ({ id: m.id, label: memberLabel(roster, m.id) }))];
    if (row?.owner_pinned && !roster.some((m) => m.id === row.owner_id)) options.splice(1, 0, { id: row.owner_id, label: `${row.owner} (off the roster)` });
    return options;
  });
  const shownRoster = $derived.by(() => {
    if (allParty) return roster;
    let room = PARTY_PREVIEW - roster.filter((m) => openParty.includes(m.id)).length;
    const shown: MemberRow[] = [];
    for (const member of roster) {
      if (openParty.includes(member.id)) shown.push(member);
      else if (room > 0) {
        shown.push(member);
        room -= 1;
      }
    }
    return shown;
  });
  const hiddenParty = $derived(roster.length - shownRoster.length);

  /** "+n" goes away once pressed; focus moves to the first member it showed. */
  async function showAllParty() {
    const before = new Set(shownRoster.map((m) => m.id));
    allParty = true;
    await tick();
    const first = roster.find((m) => !before.has(m.id));
    if (first) partyChips?.querySelector<HTMLInputElement>(`input[value="${CSS.escape(first.id)}"]`)?.focus();
  }

  function request(): FixedRequest {
    return {
      weekday: Number(weekday),
      time: time.trim(),
      bosses: [...selected, typed.trim()].filter(Boolean).join(' '),
      participants: party,
      channel_id: channel,
      note: note.trim() || null,
      // Always sent: '' keeps or restores the default; a member pins them; unchanged is no edit.
      owner_id: pickedOwner,
      decisions,
    };
  }

  async function save() {
    busy = true;
    error = '';
    ownerError = '';
    const body = request();
    const result = row
      ? await send((c) => c.patch<FixedRow>(`/api/admin/fixed/${encodeURIComponent(row.id)}`, { ...body, version: formVersion ?? undefined }))
      : await send((c) => c.post<FixedRow>('/api/admin/fixed', body));
    busy = false;
    if (!result.ok) {
      // Only `stale` means the form is out of date; `busy` and the rest leave it valid to retry as is.
      const stale = result.code === 'stale';
      step = 'edit';
      // The 422 `invalid` carries no field name; the owner's is the one that names the owner.
      if (result.code === 'invalid' && /\bowner\b/i.test(result.message)) {
        ownerError = result.message;
        await tick();
        ownerSelect?.focus();
        return;
      }
      error = stale ? `${result.message} Close and reopen this timing to edit what is saved now.` : result.message;
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
  if (wide && !leaving && !modalOpen && event.key === 'Escape') {
    event.preventDefault();
    onclose();
  }
}} />

{#snippet formBody()}
  <form id="{uid}-form" class="fixedsheet__form" onsubmit={submit} novalidate>
    <div class="fixedsheet__content">
    {#if step === 'edit'}
      <p class="eyebrow">Bosses — tap the difficulties this party runs</p>
      <!-- B_Fixed: an existing timing shows its own bosses; "All n bosses…" opens the full list. -->
      <div class="fixedsheet__bosses" data-fid="fixed-bosses">
        <BossGrid rows={shownBosses} bind:selected />
        <!-- The expander and the typed field share one line under the rows; the check reads out below them. -->
        <div class="fixedsheet__more">
          {#if row}
            <button type="button" class="linklike fixedsheet__all" aria-expanded={allBosses} onclick={() => (allBosses = !allBosses)}
              >{allBosses ? 'Only the picked bosses' : `All ${bosses.length} bosses…`}</button
            >
          {/if}
          <label class="fixedsheet__typed">
            <span>…or type them</span>
            <input bind:value={typed} placeholder="hstar, hfa" aria-describedby="{uid}-check" />
          </label>
          <span class="boss-check" id="{uid}-check" role="status">
            {#if check && 'error' in check}<span class="status status--at_risk">{check.error}</span>
            {:else if check}{#each check.bosses as boss (boss.token)}<BossTag {boss} />{/each}{/if}
          </span>
        </div>
      </div>
      <div class="fixedsheet__fields" data-fid="fixed-fields">
        <div class="field fixedsheet__day">
          <span>Day</span>
          <DayStrip days={weekdayStrip} value={Number(weekday)} label="Day" compact onpick={(value) => (weekday = value)} />
        </div>
        <!-- The Move picker's stepper (Run lengths step), still typed into; Enter submits the form. -->
        <div class="field fixedsheet__time">
          <!-- A label for the typed field only: the stepper's chevrons stay outside it. -->
          <label class="label" for="{uid}-time">Time</label>
          <TimeStepper
            id="{uid}-time"
            value={timeMinutes}
            step={timeStep}
            start={21 * 60}
            draft={time}
            placeholder="21:30"
            ontype={(text) => (time = text)}
            onchange={(minutes) => (time = hhmm(minutes))}
          />
        </div>
        <div class="field">
          <span>Owner</span>
          <Select
            bind:this={ownerSelect}
            label="Owner"
            value={pickedOwner}
            options={ownerOptions.map((o) => ({ value: o.id, label: o.label }))}
            noun="members"
            invalid={Boolean(ownerError)}
            describedby={ownerError ? `${uid}-owner-err` : undefined}
            onchange={(id) => {
              pickedOwner = id;
              ownerError = '';
            }}
          />
        </div>
        {#if ownerError}<p class="field__error fixedsheet__field-error" id="{uid}-owner-err" role="alert">{ownerError}</p>{/if}
      </div>
      <!-- B_Fixed: the home channel on its own line under the day and time. -->
      <div class="field fixedsheet__channel" data-fid="fixed-channel">
        <span>Home channel</span>
        <Select label="Home channel" bind:value={channel} options={channels.map((c) => ({ value: c.id, label: c.name }))} noun="channels" />
      </div>
      <fieldset class="field fixedsheet__party">
        <legend class="label">Party · {party.length} of {roster.length}</legend>
        <div class="run__people" data-fid="fixed-party" bind:this={partyChips}>
          {#each shownRoster as member (member.id)}
            <label class="chip">
              <input type="checkbox" value={member.id} bind:group={party} />
              {memberLabel(roster, member.id)}
            </label>
          {/each}
          {#if hiddenParty > 0}
            <button type="button" class="chip fixedsheet__more-party" aria-label="Show {hiddenParty} more member{hiddenParty === 1 ? '' : 's'}" onclick={showAllParty}>+{hiddenParty}</button>
          {/if}
        </div>
      </fieldset>
      <!-- Not on the board: the note follows the party, after the fields the board shows. -->
      <label class="field fixedsheet__note"><span>Note</span><input bind:value={note} /></label>
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
      <footer class="fixedsheet__foot" data-fid="fixed-editor-foot">
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
  <!-- A native aside (not SidePane) so the board's region name sits on the pane itself. -->
  <aside class="side-pane side-pane--fixed" class:is-leaving={leaving} inert={leaving} aria-label="Weekly timing details" data-fid="fixed-editor" onanimationend={onleft} {@attach enter(rowKey)}>
    <header class="fixedsheet__head" data-fid="fixed-editor-head">
      <div class="fixedsheet__title">
        <p class="cap">{row ? `#${row.short_id} · edit` : 'Baseline · new'}</p>
        <h2>{row ? `${row.weekday_name} ${row.time} — ${row.bosses.map((b) => b.token).join(' + ')}` : 'New weekly timing'}</h2>
      </div>
      <button class="btn btn--ghost fixedsheet__close" type="button" aria-label="Close weekly timing details" onclick={onclose}>×</button>
    </header>
    {@render formBody()}
  </aside>
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
