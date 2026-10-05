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
  import '@kanade/ui/styles/run-sheet.scss';
  import '@kanade/ui/styles/select.scss';
  import { ANSWER_MARKS, AnswerBar, AnswerChip, BossTag, enter, Icon, Modal, Select, StatusMark, WavyProgress, dayLabel, runTitle, sortRuns, whenLabel } from '@kanade/ui';
  import { swapSlots } from './planner/dropTime';
  import { directory, memberLabel } from './names/directory.svelte';
  import Name from './names/Name.svelte';
  import RunLog from './sheet/RunLog.svelte';
  import MovePicker from './sheet/MovePicker.svelte';
  import { liveRuns, namesIn, pickerRun } from './sheet/move';
  import { runCountdown } from './week/progress';
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
    step = 30,
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
    /** The Move picker's time step: Config → Run lengths default minutes. */
    step?: number;
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
  let error = $state('');
  let busy = $state(false);
  let notice = $state<{ ok: boolean; message: string; undo?: () => void } | null>(null);
  let seeded: string | null = null;
  // The pane's pill tabs (Run / Answers / Changes); the sheet stacks them.
  let paneTab = $state<'run' | 'answers' | 'changes'>('run');
  const paneTabs: Record<string, HTMLButtonElement> = {};
  // The full sheet's window tabs (HeroSheet, HeroPhone).
  type SheetTab = 'party' | 'answers' | 'cards' | 'changes';
  let sheetTab = $state<SheetTab>('party');
  // The phone sheet's "More actions" row (Swap, Preview ping, Reset to fixed).
  let moreOpen = $state(false);
  const sheetTabs: Record<string, HTMLButtonElement> = {};
  // The pane's pop-out: the same run in the full sheet (the modal used below
  // 900 px), with the pane gone meanwhile. Closing it returns to the pane on
  // the same tab, focus on the pop-out button; the run stays selected.
  let popped = $state(false);
  let popButton = $state<HTMLButtonElement>();
  // Short status words in the pane and the phone sheet; the laptop sheet spells them out.
  const short = $derived(!wide || !popped);
  // The sheet's Move view (HeroSheet's window, P_MovePhone's screen); the pane shows the picker inline.
  let moving = $state(false);
  let moveButton = $state<HTMLButtonElement>();
  const field = $derived(liveRuns(week.runs));
  const names = $derived(namesIn(week.runs));

  function popOut() {
    // The pane's tab opens the matching window tab.
    sheetTab = paneTab === 'run' ? 'party' : paneTab;
    popped = true;
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
  let swapSelect: { focus(): void } | undefined = $state();
  let adding = $state('');
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
      moving = false;
      error = '';
      notice = null;
      swapping = false;
      swapWith = '';
      paneTab = 'run';
      sheetTab = 'party';
      moreOpen = false;
    }
    if (!open) {
      seeded = null;
      popped = false;
      moving = false;
    }
  });

  const counts = $derived.by(() => {
    const all = run?.participants ?? [];
    const count = (answer: Participant['answer']) => all.filter((p) => p.answer === answer).length;
    return { on: count('yes'), out: count('no'), maybe: count('maybe'), waiting: count('waiting'), total: all.length };
  });
  // Unsaved input: the Move view or the swap picker is open.
  const dirty = $derived(moving || swapping);
  // The identity card's art: one picture, or angled slices in run order (lead
  // first), at most three; any further bosses show as their portraits only.
  const artBosses = $derived(run ? run.bosses.filter((b) => b.art).slice(0, 3) : []);
  // The pane's countdown to a run still ahead: waves over its final 24 h only.
  const until = $derived(run && wide ? runCountdown(run, week) : null);
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

  async function move(slot: Slot) {
    if (!run || saving) return;
    const id = run.id;
    error = '';
    busy = true;
    const outcome = await onmove(id, slot).finally(() => (busy = false));
    if (!outcome.ok) error = outcome.message;
    else if (wide) closeMove();
    else open = false;
  }

  function openMove() {
    moving = true;
    error = '';
  }

  // Escape anywhere in the Move view (outside the picker, which handles its own) goes back.
  function backKey(event: KeyboardEvent) {
    if (event.key !== 'Escape' || event.defaultPrevented) return;
    event.preventDefault();
    void closeMove();
  }

  // Back from the Move view: focus returns to the Move… key.
  async function closeMove() {
    if (!moving) return;
    moving = false;
    await tick();
    moveButton?.focus({ preventScroll: true });
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

  // Following a link away: the pane or sheet closes with the page it sat on.
  // A modified click opens a new tab, so this one stays.
  function leaveFor(event: MouseEvent) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    open = false;
  }

  // An action picker: the pick adds the member and the picker returns to its prompt.
  function add(id: string) {
    adding = '';
    if (run && id) void act(() => onroster(run.id, { add: id }));
  }
  const addOptions = $derived(addable.map((m) => ({ value: m.id, label: memberLabel(members, m.id) })));
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
      <Select
        class="chip__add"
        label="Add someone to {runTitle(run)} for this week"
        bind:value={adding}
        options={addOptions}
        placeholder="+ add…"
        noun="members"
        disabled={busy}
        onchange={add}
      />
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

{#snippet picker(run: Run, variant: 'pane' | 'phone', view = false)}
  {@const own = { day: run.day, time: run.status === 'otot' ? null : run.time }}
  <!-- A fresh pick per run and slot; a failed move keeps it. -->
  {#key `${run.id}@${run.day}@${own.time}`}
    <MovePicker
      days={week.days}
      subject={pickerRun(run)}
      {own}
      {field}
      {names}
      {step}
      {variant}
      legend={variant === 'phone' ? 'Day' : 'Move to'}
      aside={variant === 'phone'
        ? `boss week · ${dayLabel(week, 0)} – ${dayLabel(week, week.days.length - 1)}`
        : `from ${dayLabel(week, run.day)} · ${own.time ?? 'own time'}`}
      busy={busy || saving}
      foot
      fill={view}
      autofocus={view}
      onsubmit={(slot) => void move(slot)}
      oncancel={view ? () => void closeMove() : undefined}
    />
  {/key}
{/snippet}

{#snippet moveKey(run: Run, more = false)}
  <div class="runsheet__movekey" data-fid="week-move">
    <button
      type="button"
      class="btn btn--primary btn--key"
      bind:this={moveButton}
      aria-label="Move {runTitle(run)}…"
      disabled={busy || saving}
      onclick={openMove}
      ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
        ><path d="M5 12h14M13 6l6 6-6 6" /></svg
      >Move…</button
    >
    {#if more}
      <button
        type="button"
        class="btn btn--key runsheet__more-btn"
        aria-label="More actions"
        title="More actions"
        aria-expanded={moreOpen}
        aria-controls="{uid}-more"
        onclick={() => (moreOpen = !moreOpen)}
        ><svg class="icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" focusable="false"
          ><circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" /></svg
        ></button
      >
    {/if}
  </div>
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
  <button class="btn" type="button" disabled={busy} title="Preview this run's morning card; nothing is posted"
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
      <div class="field"
        ><span>Swap with</span>
        <Select
          bind:this={swapSelect}
          label="Swap with"
          bind:value={swapWith}
          options={swapChoices.map((other) => ({ value: other.id, label: `${whenLabel(week, other.day, other.time)} · ${runTitle(other)}` }))}
          noun="runs"
          disabled={busy || saving}
        />
      </div>
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
          aria-label={short && SHORT[value] ? label : undefined}
          disabled={busy}
          onclick={() => setStatus(value as RunStatus)}>{short ? (SHORT[value] ?? label) : label}</button
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
  {#if artBosses.length > 0}
    <div class="run__arts run__arts--{artBosses.length}" data-fid="run-art">
      {#each artBosses as boss (boss.token)}
        <span class="run__slice"><img class="run__art" src={boss.art} alt="" decoding="async" /></span>
      {/each}
    </div>
  {/if}
{/snippet}

<!-- A run from a weekly timing links to it (the app's router turns the click into a route change). -->
{#snippet timing(run: Run)}
  {#if run.fixed_id}
    <a class="runlink" href="/fixed?open={encodeURIComponent(run.fixed_id)}" onclick={leaveFor}><Icon name="pin" />View weekly timing</a>
  {/if}
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
          onclick={popOut}
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
          <header class="week-pane__art" data-fid="week-pane-art">
            {@render arts()}
            <div class="week-pane__lead">
            <p class="week-pane__when">
              <span class="week-pane__time mono">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
              <span class="cap week-pane__day">{dayLabel(week, run.day)}{#if countdown} · {countdown}{/if}</span>
            </p>
            {#if until}
              <WavyProgress
                class="wavy--inline week-pane__countdown"
                value={until.value}
                max={until.max}
                wavy={until.wavy}
                ticks={until.ticks}
                label="Countdown to {runTitle(run)}"
                text={until.text}
              />
            {/if}
            <ul class="week-pane__bosses">
              {#each run.bosses as boss (boss.token)}<li><BossTag {boss} portrait level /></li>{/each}
            </ul>
            <p class="week-pane__chips">
              <StatusMark status={run.status} words pill />
              <span class="tone tone--neutral"><b class="mono">{counts.on}/{counts.total}</b>&nbsp;on</span>
              {#if counts.out}<span class="tone tone--danger mono">{counts.out} out</span>{/if}
              {#if counts.maybe}<span class="tone tone--info">{counts.maybe} maybe</span>{/if}
              {#if counts.waiting}<span class="tone tone--neutral">{counts.waiting} waiting</span>{/if}
              {@render timing(run)}
            </p>
            <AnswerBar participants={run.participants} class="week-pane__answers" />
            </div>
          </header>
          <div class="week-pane__body">
            {@render picker(run, 'pane')}
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
  <!-- Below 900 px the sheet itself, full screen (HeroPhone); from 900 px the
       pane's pop-out (HeroSheet), whose close goes back to the pane. -->
  <Modal
    bind:open={() => (wide ? popped && open : open), (value) => (wide ? (popped = value) : (open = value))}
    title={run ? (moving && !wide ? `Move ${runTitle(run)}` : runTitle(run)) : 'Run'}
    wide
    flush
    lightDismiss={!dirty && !busy}
    className="runsheet {wide ? 'runsheet--wide' : 'runsheet--narrow'}"
  >
    {#if run}
      {@const tabs = [
        { id: 'party', label: 'Party', count: counts.total || null },
        { id: 'answers', label: 'Answers', count: counts.waiting + counts.maybe || null },
        { id: 'cards', label: 'Cards', count: run.cards.length || null },
        { id: 'changes', label: wide ? 'Who changed this' : 'Changes', count: null },
      ] as const}
      {#if moving && !wide}
        <!-- P_MovePhone: the sheet becomes the Move screen; Back (or Escape) returns to the run. -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <article class="runsheet__body runsheet__move runsheet--{run.status}" aria-label="Move {runTitle(run)}" onkeydown={backKey}>
          <div class="runsheet__moveid" data-fid="move-head">
            <button type="button" class="btn btn--ghost runsheet__back" aria-label="Back to the run" title="Back to the run" onclick={() => void closeMove()}
              ><Icon name="chevron-left" /></button
            >
            <span class="mono runsheet__movetime">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
            <span class="cap">{dayLabel(week, run.day)}</span>
            <b class="runsheet__movename">{runTitle(run)}</b>
            <span class="runsheet__moveparty">{counts.total} in party</span>
          </div>
          {@render picker(run, 'phone', true)}
          <p class="field__error run__error" role="alert">{error}</p>
        </article>
      {:else}
      <article class="runsheet__body runsheet--{run.status}" data-fid="sheet">
        <header class="runsheet__hero" data-fid="sheet-hero">
          {@render arts()}
          <p class="runsheet__when" data-fid="sheet-when">
            <span class="runsheet__time mono">{run.status === 'otot' || !run.time ? 'own time' : run.time}</span>
            <span class="cap runsheet__day">{dayLabel(week, run.day)}{countdown ? ` · ${countdown}` : ''}</span>
            <span class="runsheet__stripe" aria-hidden="true"></span>
          </p>
          <div class="runsheet__id">
            <ul class="runsheet__bosses">
              {#each run.bosses as boss (boss.token)}<li data-fid="sheet-boss"><BossTag {boss} portrait level /></li>{/each}
            </ul>
            <p class="runsheet__chips" data-fid="sheet-chips">
              <StatusMark status={run.status} words pill />
              <span class="tone tone--neutral"><b class="mono">{counts.on}/{counts.total}</b>&nbsp;on</span>
              {#if counts.out}<span class="tone tone--danger mono">{counts.out} out</span>{/if}
              {#if counts.maybe}<span class="tone tone--info">{counts.maybe} maybe</span>{/if}
              {#if counts.waiting}<span class="tone tone--neutral">{counts.waiting} waiting</span>{/if}
              {@render channel(run)}
              <span class="id">#{run.short_id}</span>
              {@render timing(run)}
            </p>
            <AnswerBar participants={run.participants} class="runsheet__answers" />
          </div>
          {#if wide}
            <div class="runsheet__side" data-fid="sheet-actions">
              {@render moveKey(run)}
              <div class="runsheet__acts">{@render actions(run)}</div>
              {@render status(run)}
            </div>
          {:else}
            <div class="runsheet__moverow">
              {@render moveKey(run, true)}
              {#if moreOpen}<div class="runsheet__acts" id="{uid}-more">{@render actions(run)}</div>{/if}
            </div>
          {/if}
          {@render swapBox(run)}
          {@render notes()}
        </header>

        {#if moving}
          <!-- HeroSheet's window turns into the Move picker; Back (or Escape) returns to the tabs. -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <section class="runsheet__win runsheet__movewin" data-fid="sheet-window" aria-labelledby="{uid}-movetitle" onkeydown={backKey}>
            <div class="runsheet__bar">
              <h3 class="runsheet__movetitle" id="{uid}-movetitle">Move {runTitle(run)}</h3>
              <button type="button" class="btn runsheet__back" onclick={() => void closeMove()}><Icon name="chevron-left" />Back to the run</button>
            </div>
            <div class="runsheet__panel runsheet__panel--move">{@render picker(run, 'pane', true)}</div>
          </section>
        {:else}
        <!-- One window: the modal's title bar is the only one; the tabs are a strip of its body (HeroSheet's `.win` tabs). -->
        <section class="runsheet__win" data-fid="sheet-window" aria-label="Run details">
          <div class="tabs__strip runsheet__bar">
            <div class="tabs__tabs" role="tablist" aria-label="Run details" data-fid="sheet-tabs">
              {#each tabs as tab, index (tab.id)}
                <button
                  type="button"
                  role="tab"
                  class="tabs__tab"
                  id="{uid}-stab-{tab.id}"
                  aria-selected={sheetTab === tab.id}
                  aria-controls="{uid}-spanel"
                  tabindex={sheetTab === tab.id ? 0 : -1}
                  bind:this={sheetTabs[tab.id]}
                  onclick={() => (sheetTab = tab.id)}
                  onkeydown={(event) => {
                    const step = { ArrowRight: 1, ArrowLeft: -1 }[event.key];
                    if (!step) return;
                    event.preventDefault();
                    const next = tabs[(index + step + tabs.length) % tabs.length]!;
                    sheetTab = next.id;
                    sheetTabs[next.id]?.focus();
                  }}
                  >{tab.label}{#if tab.count}<span class="tabs__count">{tab.count}</span>{/if}</button
                >
              {/each}
            </div>
          </div>
          <div class="runsheet__panel runsheet__panel--{sheetTab}" role="tabpanel" id="{uid}-spanel" aria-labelledby="{uid}-stab-{sheetTab}">
            {#if sheetTab === 'party'}
              <ul class="runsheet__party" aria-label="Party" data-fid="week-party">
                {#each run.participants as person (person.id)}
                  <li class="slot slot--{person.answer}">
                    <span class="slot__mark" aria-hidden="true">{ANSWER_MARKS[person.answer].mark}</span>
                    <span class="slot__text"
                      ><b class="slot__name">{who(person)}</b><span class="slot__state">{ANSWER_MARKS[person.answer].word}</span></span
                    >
                    <button
                      type="button"
                      class="chip__x slot__x"
                      aria-label="Take {who(person)} off this run for this week only"
                      disabled={busy}
                      onclick={() => void act(() => onroster(run.id, { remove: person.id }))}>×</button
                    >
                  </li>
                {/each}
                {#if addable.length > 0}
                  <li class="slot slot--open">
                    <span class="slot__mark" aria-hidden="true">+</span>
                    <Select
                      class="slot__add"
                      label="Add someone to {runTitle(run)} for this week"
                      bind:value={adding}
                      options={addOptions}
                      placeholder="Add someone…"
                      noun="members"
                      disabled={busy}
                      onchange={add}
                    />
                  </li>
                {/if}
              </ul>
              {#if wide}
                <aside class="runsheet__aside" aria-label="This week" data-fid="sheet-aside">{@render thisWeek(run)}</aside>
              {/if}
            {:else if sheetTab === 'answers'}
              <div class="answers__body">{@render answerRows(run)}</div>
            {:else if sheetTab === 'cards'}
              {#if run.roster_change || run.cards.length > 0}{@render thisWeek(run)}{:else}<p class="note">No cards for this run yet.</p>{/if}
            {:else}
              {#key run.id}{@render changes(run)}{/key}
            {/if}
          </div>
          {#if !wide}
            <div class="runsheet__foot" data-fid="sheet-foot">{@render status(run)}</div>
          {/if}
        </section>
        {/if}
      </article>
      {/if}
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
