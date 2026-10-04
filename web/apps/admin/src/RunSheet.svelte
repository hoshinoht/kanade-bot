<!--
  One run's sheet, v4 partials/run.html in a modal: entry art under the veil,
  bosses with portraits and levels, the answer tally, answer chips with remove
  and add, the reminder-card timeline, Move + Preview ping, the status control
  and the folded answers editor. Every action is server-confirmed and reported
  in the sheet's own status line: a modal dialog makes the page's toast region
  inert, so results and Undo must live inside it. Failed input stays in its field.
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { Member, Participant, Run, RunStatus, Week } from '@kanade/api-types';
  import { AnswerChip, BossTag, enter, Icon, Modal, StatusMark, dayLabel, runTitle, sortRuns, whenLabel } from '@kanade/ui';
  import { swapSlots } from './planner/dropTime';
  import { directory, memberLabel } from './names/directory.svelte';
  import Name from './names/Name.svelte';
  import RunLog from './sheet/RunLog.svelte';
  import { parseWhen } from './sheet/parseWhen';
  import { STATUS_LABELS, type MoveOutcome } from './store.svelte';
  import type { Slot } from './planner/keyboardMove';

  let {
    open = $bindable(false),
    run,
    week,
    members,
    onmove,
    onswap,
    onstatus,
    onrsvp,
    onroster,
    onping,
    onreset,
    onreread,
    saving = false,
    wide = false,
    countdown = null,
    onclose,
  }: {
    open: boolean;
    run: Run | null;
    week: Week;
    members: Member[];
    onmove: (runId: string, to: Slot) => Promise<MoveOutcome>;
    /** Exchange this run's slot with another run of the same boss week. */
    onswap: (runId: string, withId: string) => Promise<MoveOutcome>;
    onstatus: (runId: string, status: RunStatus) => Promise<MoveOutcome>;
    onrsvp: (runId: string, memberId: string, answer: 'yes' | 'no' | 'clear') => Promise<MoveOutcome>;
    onroster: (runId: string, change: { add?: string; remove?: string }) => Promise<MoveOutcome>;
    onping: (runId: string) => Promise<MoveOutcome>;
    onreset: (runId: string) => Promise<MoveOutcome>;
    onreread: (run: Run) => Promise<MoveOutcome>;
    saving?: boolean;
    /** A side pane in the Week window (gate G4) instead of the full-screen sheet. */
    wide?: boolean;
    /** "10 h" when this run is the next one up. */
    countdown?: string | null;
    /** The pane's close button and Escape. */
    onclose?: () => void;
  } = $props();

  const ANSWERS = [
    ['yes', 'On'],
    ['no', 'Out'],
    ['clear', 'Clear'],
  ] as const;
  const uid = $props.id();
  // The pane's status group is one row (B_WeekSel); each short word sits inside its full name.
  const SHORT: Record<string, string> = { otot: 'Own', cancelled: 'Cancel' };
  let to = $state('');
  let error = $state('');
  let busy = $state(false);
  let notice = $state<{ ok: boolean; message: string; undo?: () => void } | null>(null);
  let seeded: string | null = null;
  // The pane's pill tabs (Run / Answers / Changes); the sheet stacks them.
  let paneTab = $state<'run' | 'answers' | 'changes'>('run');
  const paneTabs: Record<string, HTMLButtonElement> = {};
  // The pane's pop-out: the same run in the full sheet (the modal used below
  // 840 px), with the pane gone meanwhile. Closing it returns to the pane on
  // the same tab, focus on the pop-out button; the run stays selected.
  let popped = $state(false);
  let popButton = $state<HTMLButtonElement>();
  let sheetBody = $state<HTMLElement>();

  async function popOut() {
    popped = true;
    await tick();
    // The pane's tab is the sheet's open section: bring it into view.
    if (paneTab !== 'run') requestAnimationFrame(() => sheetBody?.querySelector(paneTab === 'answers' ? '.run__answers' : '.runlog')?.scrollIntoView({ block: 'nearest' }));
  }

  // Back from the pop-out (the sheet unmounts with its branch, so no close
  // event to hang this on): focus returns to the pane's pop-out button.
  let wasPopped = false;
  $effect(() => {
    if (popped) wasPopped = true;
    else if (wasPopped) {
      wasPopped = false;
      if (untrack(() => open)) void tick().then(() => popButton?.focus({ preventScroll: true }));
    }
  });

  // "Swap timing with…": pick another live run of this boss week, review
  // where both land, then confirm. Undo comes with the toast like a move.
  let swapping = $state(false);
  let swapWith = $state('');
  let swapSelect: HTMLSelectElement | undefined = $state();
  let swapButton: HTMLButtonElement | undefined = $state();
  const swapChoices = $derived(
    run ? sortRuns(week.runs.filter((r) => r.id !== run.id && r.status !== 'done' && r.status !== 'cancelled')) : [],
  );
  const swapOther = $derived(swapChoices.find((r) => r.id === swapWith) ?? null);
  const swapPreview = $derived.by(() => {
    if (!run || !swapOther) return null;
    const to = swapSlots(run, run.status === 'otot', swapOther, swapOther.status === 'otot');
    return {
      daysOnly: to.daysOnly,
      mine: whenLabel(week, to.a.day, to.a.time),
      theirs: whenLabel(week, to.b.day, to.b.time),
    };
  });

  // A fresh field per opened run; a failed move keeps what was typed.
  $effect(() => {
    if (open && run && seeded !== run.id) {
      seeded = run.id;
      to = '';
      error = '';
      notice = null;
      swapping = false;
      swapWith = '';
      paneTab = 'run';
    }
    if (!open) {
      seeded = null;
      popped = false;
    }
  });

  const counts = $derived.by(() => {
    const all = run?.participants ?? [];
    const count = (answer: Participant['answer']) => all.filter((p) => p.answer === answer).length;
    return { on: count('yes'), out: count('no'), maybe: count('maybe'), waiting: count('waiting'), total: all.length };
  });
  // Unsaved input: the Move field holds a target, or the swap picker is open.
  const dirty = $derived(to.trim() !== '' || swapping);
  const artBosses = $derived(run ? run.bosses.filter((b) => b.art) : []);
  const addable = $derived(run ? members.filter((m) => !run.participants.some((p) => p.id === m.id)) : []);
  // A twin reads "Ren (2)", never its id.
  const who = (p: Participant) => memberLabel(run?.participants ?? [], p.id);

  async function act(work: () => Promise<MoveOutcome>, undo?: () => Promise<MoveOutcome>) {
    busy = true;
    try {
      const outcome = await work();
      notice = { ...outcome, undo: outcome.ok && undo ? () => void act(undo) : undefined };
      return outcome;
    } finally {
      busy = false;
    }
  }

  // aria-disabled, not disabled: the re-read takes seconds and focus must stay put.
  function rereadChannel(target: Run) {
    if (busy) return;
    notice = { ok: true, message: `Re-reading ${target.channel}…` };
    void act(() => onreread(target));
  }

  // Status changes are reversible, so they get Undo here rather than v4's confirm dialog.
  function setStatus(status: RunStatus) {
    if (!run || run.status === status) return;
    const id = run.id;
    const before = run.status;
    void act(() => onstatus(id, status), before === 'at_risk' ? undefined : () => onstatus(id, before));
  }

  async function move(event: SubmitEvent) {
    event.preventDefault();
    if (!run || saving) return;
    const parsed = parseWhen(to, week.days, { day: run.day, time: run.time });
    if (!parsed.ok) {
      error = parsed.message;
      return;
    }
    const id = run.id;
    busy = true;
    const outcome = await onmove(id, parsed.slot).finally(() => (busy = false));
    if (!outcome.ok) error = outcome.message;
    else if (wide) to = '';
    else open = false;
  }

  async function openSwap() {
    swapping = true;
    swapWith = swapChoices[0]?.id ?? '';
    await tick();
    swapSelect?.focus();
  }

  async function closeSwap() {
    swapping = false;
    await tick();
    swapButton?.focus();
  }

  async function confirmSwap() {
    if (!run || !swapOther || saving) return;
    busy = true;
    const outcome = await onswap(run.id, swapOther.id).finally(() => (busy = false));
    if (!outcome.ok) error = outcome.message;
    else if (wide) swapping = false;
    else open = false;
  }

  function add(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const id = select.value;
    select.value = '';
    if (run && id) void act(() => onroster(run.id, { add: id }));
  }
</script>

<svelte:window
  onkeydown={(event) => {
    // Escape closes the pane unless something nearer used it (a planner lift, a popover, a dialog).
    if (!wide || popped || !open || event.key !== 'Escape' || event.defaultPrevented || document.querySelector('dialog[open]')) return;
    event.preventDefault();
    onclose?.();
  }}
/>

{#snippet people(run: Run)}
  <div class="run__people" data-fid="week-party">
    {#each run.participants as person (person.id)}
      <AnswerChip participant={person} label={who(person)}>
        <button
          type="button"
          class="chip__x"
          aria-label="Take {who(person)} off this run for this week only"
          disabled={busy}
          onclick={() => void act(() => onroster(run.id, { remove: person.id }))}>×</button
        >
      </AnswerChip>
    {/each}
    {#if addable.length > 0}
      <select class="chip__add" aria-label="Add someone to {runTitle(run)} for this week" onchange={add} disabled={busy}>
        <option value="">+ add…</option>
        {#each addable as member (member.id)}<option value={member.id}>{memberLabel(members, member.id)}</option>{/each}
      </select>
    {/if}
  </div>
{/snippet}

{#snippet channel(run: Run)}
  <span class="chanmark"
    >{run.channel}
    <button
      type="button"
      class="chanmark__btn"
      aria-label="Re-read {run.channel} from Discord and propose any changes"
      aria-disabled={busy}
      onclick={() => rereadChannel(run)}><Icon name="refresh-cw" /></button
    ></span
  >
{/snippet}

{#snippet roster(run: Run)}
  {#if run.roster_change}{#each run.roster_change.out as person (person.id)}{` −${directory.label('member', person.id, person.name)}`}{/each}{#each run.roster_change.in as person (person.id)}{` +${directory.label('member', person.id, person.name)}`}{/each}{/if}
{/snippet}

{#snippet thisWeek(run: Run)}
  {#if run.roster_change}
    <p class="run__meta">
      <span class="chip chip--waiting">this week:{@render roster(run)}</span>
    </p>
  {/if}
  {#if run.cards.length > 0}
    <p class="run__cards">
      <a class="run__cards-label" href="/reminders?run={encodeURIComponent(run.id)}" aria-label="Cards: every reminder for this run">Cards</a>
      {@render cardLinks(run)}
    </p>
  {/if}
{/snippet}

<!-- The pane's one line (B_WeekSel): "This week: +Ren · cards morning 00:15 T-1h 21:00". -->
{#snippet paneWeek(run: Run)}
  {#if run.roster_change || run.cards.length > 0}
    <p class="week-pane__week run__cards">
      {#if run.roster_change}<span>This week:{@render roster(run)}</span>{/if}{#if run.roster_change && run.cards.length > 0}<span aria-hidden="true"> · </span>{/if}{#if run.cards.length > 0}<a
          class="run__cards-label"
          href="/reminders?run={encodeURIComponent(run.id)}"
          aria-label="Cards: every reminder for this run">cards</a
        >
        {@render cardLinks(run)}{/if}
    </p>
  {/if}
{/snippet}

{#snippet cardLinks(run: Run)}
      {#each run.cards as card (card.label)}
        {#if card.state === 'posted' && card.url}
          <a class="cardlink cardlink--posted" href={card.url} target="_blank" rel="noopener noreferrer"
            title="Open the {card.label} card in Discord (posted {card.at})"
            >{card.label} <span class="cardlink__at">{card.at}</span><span class="cardlink__out"><Icon name="external-link" /></span
            ><span class="vh">(posted, opens Discord)</span></a
          >
        {:else if card.state === 'skipped'}
          <span class="cardlink cardlink--skipped" title="Due {card.at}, retired without posting">{card.label} skipped</span>
        {:else}
          <span class="cardlink cardlink--queued" title="Not posted yet; fires at {card.at}"
            >{card.label} <span class="cardlink__at">{card.at}</span><span class="vh">(queued)</span></span
          >
        {/if}
      {/each}
{/snippet}

{#snippet moveForm(run: Run)}
  <form onsubmit={move} novalidate data-fid="week-move">
    <input
      bind:value={to}
      placeholder="wed 21:30"
      size="10"
      aria-label="Move {runTitle(run)} to a new day and time"
      aria-invalid={error ? 'true' : undefined}
      aria-describedby="{uid}-error"
    />
    <button class="btn" class:btn--primary={wide} type="submit" disabled={busy || saving}>Move</button>
  </form>
{/snippet}

{#snippet actions(run: Run)}
  {#if !['done', 'cancelled'].includes(run.status)}
    <button
      class="btn"
      type="button"
      bind:this={swapButton}
      disabled={busy || saving || swapChoices.length === 0}
      aria-expanded={swapping}
      aria-controls="{uid}-swap"
      aria-label={wide ? 'Swap timing with…' : undefined}
      title={wide ? 'Swap timing with…' : undefined}
      onclick={() => (swapping ? void closeSwap() : void openSwap())}>{wide ? 'Swap…' : 'Swap timing with…'}</button
    >
  {/if}
  <button class="btn" type="button" disabled={busy} title="Post this run's morning card now, as a TEST message"
    onclick={() => void act(() => onping(run.id))}>Preview ping</button
  >
  {#if run.fixed_id && run.amended && !['done', 'cancelled'].includes(run.status)}
    <!-- v5: undo this week's amendments in one step (day, time and roster). -->
    <button class="btn" type="button" disabled={busy} title="Put this run back on its weekly timing"
      onclick={() => void act(() => onreset(run.id))}>Reset to fixed</button
    >
  {/if}
{/snippet}

{#snippet swapBox(run: Run)}
  {#if swapping}
    <div class="swap" id="{uid}-swap" role="group" aria-labelledby="{uid}-swap-title">
      <p class="swap__title" id="{uid}-swap-title">Swap {runTitle(run)}'s timing with another run this boss week</p>
      <label class="field"
        ><span>Swap with</span>
        <select bind:this={swapSelect} bind:value={swapWith} disabled={busy || saving}>
          {#each swapChoices as other (other.id)}
            <option value={other.id}>{whenLabel(week, other.day, other.time)} · {runTitle(other)}</option>
          {/each}
        </select>
      </label>
      {#if swapPreview && swapOther}
        <p class="swap__preview" aria-live="polite">
          {runTitle(run)} → <strong>{swapPreview.mine}</strong>; {runTitle(swapOther)} → <strong>{swapPreview.theirs}</strong>{swapPreview.daysOnly
            ? ' (own time: only the days change)'
            : ''}.
        </p>
      {/if}
      <div class="swap__actions">
        <button class="btn btn--primary" type="button" disabled={busy || saving || !swapOther} onclick={() => void confirmSwap()}>Swap</button>
        <button class="btn" type="button" disabled={busy} onclick={() => void closeSwap()}>Cancel</button>
      </div>
    </div>
  {/if}
  <p class="field__error run__error" id="{uid}-error" role="alert">{error}</p>
{/snippet}

{#snippet status(run: Run)}
  <div class="statusbar" role="group" aria-labelledby="{uid}-status">
    <span class="statusbar__label" id="{uid}-status">Status</span>
    <div class="seg seg--status" data-fid="week-status">
      {#each Object.entries(STATUS_LABELS) as [value, label] (value)}
        <button
          type="button"
          class="seg__btn"
          aria-pressed={run.status === value}
          aria-label={wide && SHORT[value] ? label : undefined}
          disabled={busy}
          onclick={() => setStatus(value as RunStatus)}>{wide ? (SHORT[value] ?? label) : label}</button
        >
      {/each}
    </div>
    {#if run.status === 'at_risk'}
      <span class="statusbar__note">someone said no — pick a status to settle it</span>
    {/if}
  </div>
{/snippet}

{#snippet notes()}
  <p class="sheet__notice" class:sheet__notice--error={notice && !notice.ok} role="status">
    {#if notice}
      {notice.message}
      {#if notice.undo}<button type="button" class="btn" onclick={notice.undo} disabled={busy}>Undo</button>{/if}
    {/if}
  </p>
{/snippet}

{#snippet answerRows(run: Run)}
  <p class="answers__hint">Records the answer as if they had reacted — the run's cards update in Discord.</p>
  {#each run.participants as person (person.id)}
    <div class="answers__row">
      <span class="answers__who"><Name kind="member" id={person.id} name={who(person)} /></span>
      <div class="seg seg--answer" role="group" aria-label="Answer for {who(person)} on {runTitle(run)}">
        {#each ANSWERS as [value, label] (value)}
          <button
            type="button"
            class="seg__btn"
            aria-pressed={value === 'clear' ? false : person.answer === value}
            disabled={busy || (value === 'clear' && person.answer === 'waiting')}
            onclick={() => void act(() => onrsvp(run.id, person.id, value))}>{label}</button
          >
        {/each}
      </div>
    </div>
  {/each}
{/snippet}

{#snippet changes(run: Run)}
  <RunLog runId={run.id} title={runTitle(run)} {members} timezone={week.timezone} now={week.generated_at} channel={{ id: run.channel_id, name: run.channel }} />
{/snippet}

{#snippet arts()}
  {#each artBosses as boss, index (boss.key)}
    <img
      class="run__art"
      class:run__art--lead={artBosses.length > 1 && index === 0}
      class:run__art--second={artBosses.length > 1 && index > 0}
      src={boss.art}
      alt=""
      decoding="async"
    />
  {/each}
{/snippet}

{#if wide && !popped}
  {#if open && run}
    {@const tabs = [
      { id: 'run', label: 'Run', count: null },
      { id: 'answers', label: 'Answers', count: counts.waiting + counts.maybe || null },
      { id: 'changes', label: 'Changes', count: null },
    ] as const}
    <aside class="side-pane week-pane run--{run.status}" aria-label={runTitle(run)} data-fid="week-pane" {@attach enter(run.id)}>
      <div class="week-pane__head" data-fid="week-pane-head">
        <div class="week-pane__tabs" role="tablist" aria-label="Run details">
          {#each tabs as tab, index (tab.id)}
            <button
              type="button"
              role="tab"
              class="ptab"
              id="{uid}-ptab-{tab.id}"
              aria-selected={paneTab === tab.id}
              aria-controls="{uid}-ppanel"
              tabindex={paneTab === tab.id ? 0 : -1}
              bind:this={paneTabs[tab.id]}
              onclick={() => (paneTab = tab.id)}
              onkeydown={(event) => {
                const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
                if (!step) return;
                event.preventDefault();
                const next = tabs[(index + step + tabs.length) % tabs.length]!;
                paneTab = next.id;
                paneTabs[next.id]?.focus();
              }}
              >{tab.label}{#if tab.count}<span class="ptab__count mono">{tab.count}</span>{/if}</button
            >
          {/each}
        </div>
        <button
          type="button"
          class="btn btn--ghost week-pane__close week-pane__popout"
          aria-label="Open in a larger view"
          title="Open in a larger view"
          bind:this={popButton}
          onclick={() => void popOut()}
          ><svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            focusable="false"
            ><polyline points="15 3 21 3 21 9" /><polyline points="9 21 3 21 3 15" /><line x1="21" y1="3" x2="14" y2="10" /><line
              x1="3"
              y1="21"
              x2="10"
              y2="14"
            /></svg
          ></button
        >
        <button type="button" class="btn btn--ghost week-pane__close" aria-label="Close {runTitle(run)}" onclick={() => onclose?.()}><Icon name="x" /></button>
      </div>
      <div class="week-pane__panel" role="tabpanel" id="{uid}-ppanel" aria-labelledby="{uid}-ptab-{paneTab}">
        {#if paneTab === 'run'}
          {@render arts()}
          <header class="week-pane__art" data-fid="week-pane-art">
            <div class="week-pane__lead">
            <p class="week-pane__when">
              <span class="week-pane__time mono">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
              <span class="cap week-pane__day">{dayLabel(week, run.day)}{#if countdown} · {countdown}{/if}</span>
            </p>
            <ul class="week-pane__bosses">
              {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait level /></li>{/each}
            </ul>
            <p class="week-pane__chips">
              <StatusMark status={run.status} words pill />
              <span class="tone tone--neutral"><b class="mono">{counts.on}/{counts.total}</b>&nbsp;on</span>
              {#if counts.out}<span class="tone tone--danger mono">{counts.out} out</span>{/if}
              {#if counts.maybe}<span class="tone tone--info">{counts.maybe} maybe</span>{/if}
              {#if counts.waiting}<span class="tone tone--neutral">{counts.waiting} waiting</span>{/if}
            </p>
            </div>
          </header>
          <div class="week-pane__body">
            {@render moveForm(run)}
            <div class="week-pane__actions">{@render actions(run)}</div>
            {@render swapBox(run)}
            <p class="cap week-pane__label">Party · {@render channel(run)} <span class="id">#{run.short_id}</span></p>
            {@render people(run)}
            {@render paneWeek(run)}
            {@render status(run)}
            {@render notes()}
          </div>
        {:else if paneTab === 'answers'}
          <div class="week-pane__body answers__body">
            {@render answerRows(run)}
            {@render notes()}
          </div>
        {:else}
          <div class="week-pane__body">
            {#key run.id}{@render changes(run)}{/key}
          </div>
        {/if}
      </div>
    </aside>
  {/if}
{:else}
  <!-- A backdrop click closes it unless something is still unsaved: a typed Move
       target or an open swap picker (everything else saves on press), or a
       change still on its way. Escape and × close it as before. -->
  <!-- Below 840 px the sheet itself; from 840 px the pane's pop-out, whose close goes back to the pane. -->
  <Modal
    bind:open={() => (wide ? popped && open : open), (value) => (wide ? (popped = value) : (open = value))}
    title={run ? runTitle(run) : 'Run'}
    wide
    flush
    lightDismiss={!dirty && !busy}
  >
    {#if run}
      <article class="run run--{run.status}" bind:this={sheetBody}>
        {@render arts()}
        <div class="run__time">
          {run.status === 'otot' || !run.time ? 'own time' : run.time}
          <small>{dayLabel(week, run.day)}</small>
        </div>

        <div>
          <div class="run__bosses">
            {#if run.bosses.length > 1}
              <ul class="bosslist">
                {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait level /></li>{/each}
              </ul>
            {:else}
              {#each run.bosses as boss (boss.token)}<BossTag {boss} portrait level />{/each}
            {/if}
          </div>
          <div class="run__meta">
            <StatusMark status={run.status} words pill />
            <span class="mono">{counts.on}/{counts.total} on</span>
            {#if counts.out}<span class="tone tone--danger mono">{counts.out} out</span>{/if}
            {#if counts.maybe}<span class="tone tone--info mono">{counts.maybe} maybe</span>{/if}
            {#if counts.waiting}<span class="tone tone--neutral mono">{counts.waiting} waiting</span>{/if}
            {@render channel(run)}
            <span class="id">#{run.short_id}</span>
          </div>
          {@render people(run)}
          {@render thisWeek(run)}
        </div>

        <div class="run__actions">
          {@render moveForm(run)}
          {@render actions(run)}
        </div>
        {@render swapBox(run)}
        {@render status(run)}
        {@render notes()}

        <details class="answers run__answers" open={popped && paneTab === 'answers'}>
          <summary class="btn answers__summary"><Icon name="chevron-right" /> Answers — set who’s in or out</summary>
          <div class="answers__body">{@render answerRows(run)}</div>
        </details>
        {#key run.id}{@render changes(run)}{/key}
      </article>
    {/if}
  </Modal>
{/if}

<style>
  .swap {
    grid-column: 1 / -1;
    display: grid;
    gap: 0.4rem;
    margin: 0.3rem 0 0;
    padding: 0.6rem 0.75rem;
    border: 2px solid var(--line);
    border-radius: var(--r);
    background: var(--raise);
  }

  .swap__title,
  .swap__preview {
    margin: 0;
    font-size: var(--fs-small);
  }

  .swap__title {
    font-weight: 700;
  }

  .swap__actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .run__error:empty {
    display: none;
  }

  .run__error {
    grid-column: 1 / -1;
    margin: 0;
  }

  .sheet__notice {
    grid-column: 1 / -1;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.3rem 0.7rem;
    margin: 0.3rem 0 0;
    font-size: var(--fs-small);
    color: var(--ok-text);
  }

  .sheet__notice--error {
    color: var(--risk-text);
  }
</style>
