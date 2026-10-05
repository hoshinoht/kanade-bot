<script lang="ts">
  import { tick } from 'svelte';
  import type { Run, Week } from '@kanade/api-types';
  import { Icon, LiveRegion, WeekRail, dayLabel, runTitle, scrollEdges, sortRuns, whenLabel } from '@kanade/ui';
  import { clashes, dropTime, swapSlots, timedOthers, zoneAt, type DropTime, type TimedRun } from './dropTime';
  import { IDLE, cancel, describeSlot, onKey, type LiftState, type MovableRun, type Slot } from './keyboardMove';
  import PlannerCard from './PlannerCard.svelte';
  import PlannerColumn from './PlannerColumn.svelte';
  import type { PointerDrag } from './pointerDrag';

  let {
    week,
    helpId,
    onmove,
    onswap,
    onopen,
    onhold,
    onreread,
    rereadOff = null,
    busyChannels,
    saving = false,
    step = 30,
    allRuns,
    selectedRun = null,
  }: {
    week: Week;
    /** The week header's move instructions: every movable card is described by them. */
    helpId: string;
    onmove: (runId: string, to: Slot) => void;
    /** Exchange two runs' slots (a drop on a card, or S during a keyboard lift). */
    onswap: (runId: string, withId: string) => void;
    onopen: (run: Run) => void;
    onhold: (holding: boolean) => void;
    onreread?: (run: Run) => void;
    /** Re-reading is off: the server's sentence and the id of the note showing it. */
    rereadOff?: { note: string; id: string } | null;
    busyChannels?: Set<string>;
    /** A planner write is pending; starting another one would make rollback unsafe. */
    saving?: boolean;
    /** Keyboard Up/Down step: Config → Run lengths default minutes. */
    step?: number;
    /** Every run of the week, filtered out or not: a clash with a hidden run is still a clash. */
    allRuns?: Run[];
    selectedRun?: string | null;
  } = $props();

  let lift = $state<LiftState>(IDLE);
  let dragging = $state<string | null>(null);
  let overDay = $state<number | null>(null);
  // Two regions: passing hover chatter during a pointer drag is polite, so it
  // never cuts off anything else; results and keyboard steps (direct answers to
  // a key press, each superseding the last) are assertive.
  let urgent = $state('');
  let passing = $state('');
  let flip = false;
  let ghost: HTMLDivElement;

  // Runs that can still clash: finished and cancelled ones are left out.
  const ACTIVE = (run: Run) => run.status !== 'done' && run.status !== 'cancelled';
  // A member who answered no is not playing, so they cannot clash.
  function timed(run: Run, slot?: Slot): TimedRun {
    return {
      id: run.id,
      day: slot?.day ?? run.day,
      time: run.status === 'otot' ? null : slot ? slot.time : run.time,
      minutes: run.minutes ?? 0,
      members: run.participants.filter((p) => p.answer !== 'no').map((p) => p.id),
    };
  }
  const everyRun = $derived(allRuns ?? week.runs);
  const active = $derived(everyRun.filter(ACTIVE).map((r) => timed(r)));
  const ctx = $derived({
    dayLabel: (d: number) => dayLabel(week, d),
    lastDay: week.days.length - 1,
    step,
    others: (d: number) => timedOthers(active, d, lift.kind === 'lifted' ? lift.runId : ''),
    occupant: (slot: Slot, movingId: string) => {
      const here = everyRun.find((r) => ACTIVE(r) && r.id !== movingId && r.day === slot.day && r.time === slot.time);
      return here ? { id: here.id, label: runTitle(here) } : null;
    },
  });

  /**
   * "Asahi in HFA 21:00" for a run at a slot, or null when nobody is
   * double-booked; `moved` places another run elsewhere first (a swap).
   */
  function clashText(run: Run, slot?: Slot, moved?: { run: Run; slot: Slot }): string | null {
    const field = moved ? active.map((r) => (r.id === moved.run.id ? timed(moved.run, moved.slot) : r)) : active;
    const found = clashes(timed(run, slot), field);
    if (!found.length) return null;
    const names = new Map(everyRun.flatMap((r) => r.participants).map((p) => [p.id, p.name]));
    return found
      .map((c) => {
        const other = everyRun.find((r) => r.id === c.with.id);
        const who = c.members.map((id) => names.get(id) ?? id).join(', ');
        return `${who} in ${other ? runTitle(other) : 'another run'} ${c.with.time}`;
      })
      .join('; ');
  }
  /** The swap's wording: "Swap with HCarling + HStar: 20:00 ⇄ 22:00", or days only when either is own-time. */
  function swapText(a: Run, b: Run): string {
    const to = swapSlots(a, a.status === 'otot', b, b.status === 'otot');
    if (to.daysOnly) return `Swap days with ${runTitle(b)}: ${dayLabel(week, a.day)} ⇄ ${dayLabel(week, b.day)}, times kept`;
    const at = (r: Run) => (a.day === b.day ? (r.time ?? 'own time') : `${dayLabel(week, r.day)} ${r.time ?? 'own time'}`);
    return `Swap with ${runTitle(b)}: ${at(a)} ⇄ ${at(b)}`;
  }

  /** Clashes of both runs once swapped, or null. */
  function swapClash(a: Run, b: Run): string | null {
    const to = swapSlots(a, a.status === 'otot', b, b.status === 'otot');
    const found = [clashText(a, to.a, { run: b, slot: to.b }), clashText(b, to.b, { run: a, slot: to.a })].filter(Boolean);
    return found.length ? [...new Set(found)].join('; ') : null;
  }

  // Every card that clashes as it stands (overlap alone is allowed).
  const cardClash = $derived(new Map(week.runs.filter(ACTIVE).map((r) => [r.id, clashText(r)] as const)));
  const byDay = $derived(week.days.map((day) => sortRuns(week.runs.filter((r) => r.day === day.index))));
  const draggedRun = $derived(dragging ? week.runs.find((r) => r.id === dragging) : undefined);

  function say(message: string, polite = false) {
    // Alternate a trailing space so a repeated sentence is still announced.
    flip = !flip;
    const text = message + (flip ? '\u00a0' : '');
    if (polite) passing = text;
    else urgent = text;
  }

  function movable(run: Run): MovableRun {
    return { id: run.id, day: run.day, time: run.time, label: runTitle(run), minutes: run.minutes };
  }

  async function focusHandle(runId: string) {
    await tick();
    document.querySelector<HTMLElement>(`[data-handle="${CSS.escape(runId)}"]`)?.focus();
  }

  function commit(runId: string, to: Slot) {
    onmove(runId, to);
    void focusHandle(runId);
  }

  function commitSwap(runId: string, withId: string) {
    onswap(runId, withId);
    void focusHandle(runId);
  }

  // A keyboard drop (Space fires its click on keyup) or a pointer drop must
  // not also open the sheet on the card it landed on.
  let quietUntil = 0;
  function open(run: Run) {
    if (performance.now() < quietUntil) return;
    onopen(run);
  }

  function handleKey(event: KeyboardEvent, run: Run) {
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    if (saving && lift.kind === 'idle' && event.key.toLowerCase() === 'm') {
      event.preventDefault();
      say('Saving the last change…', true);
      return;
    }
    const outcome = onKey(lift, event.key, movable(run), ctx, event.shiftKey);
    if (!outcome.handled) return;
    event.preventDefault();
    quietUntil = performance.now() + 400;
    lift = outcome.state;
    // A step that lands on a clash says so with the step.
    const clash = lift.kind === 'lifted' && !outcome.commit ? clashText(run, lift.at) : null;
    if (outcome.announce) say(clash ? `${outcome.announce} Clash: ${clash}.` : outcome.announce);
    // The move starts while the hold is still on, so it is sent against the
    // week the lift began on: a poll buffered meanwhile (another admin's
    // change) is not applied first, and the server answers with a conflict
    // instead of the drop overwriting it. Releasing the hold then flushes.
    if (outcome.commit) commit(outcome.commit.runId, outcome.commit.to);
    if (outcome.swap) commitSwap(outcome.swap.runId, outcome.swap.withId);
    onhold(lift.kind === 'lifted');
  }

  function handleBlur(run: Run) {
    if (lift.kind !== 'lifted' || lift.runId !== run.id) return;
    const outcome = cancel(lift, movable(run), ctx);
    lift = outcome.state;
    onhold(false);
    if (outcome.announce) say(outcome.announce);
  }

  function previewFor(day: number): { text: string; clash: string | null } | null {
    const current = lift;
    if (current.kind === 'lifted' && current.at.day === day) {
      const run = week.runs.find((r) => r.id === current.runId);
      if (!run) return null;
      // On another run's slot the keyboard can swap (S) or drop beside it (Enter).
      const here = ctx.occupant(current.at, run.id);
      const other = here ? everyRun.find((r) => r.id === here.id) : undefined;
      if (other) return { text: `${current.at.time ?? 'own time'} ${runTitle(run)} · S: ${swapText(run, other)}`, clash: swapClash(run, other) };
      return { text: `${current.at.time ?? 'own time'} ${runTitle(run)}`, clash: clashText(run, current.at) };
    }
    return null;
  }

  // The board renders (and moves by keyboard) at once; pointer dragging
  // hydrates when the browser is idle or a pointer reaches the board.
  let engine = $state<PointerDrag | null>(null);
  let hydrating = false;
  let destroyed = false;
  // A press that lands before the engine (touch has no pointerover first) is
  // held and replayed once the cards bind, so that very press can still drag.
  let pending: PointerEvent | null = null;
  function hold(event: Event) {
    if (engine) return;
    if (event instanceof PointerEvent && event.isPrimary && event.button === 0) pending = event;
    hydrate();
  }
  function release(event: Event) {
    if (event instanceof PointerEvent && event.pointerId === pending?.pointerId) pending = null;
  }
  function hydrate() {
    if (hydrating || destroyed) return;
    hydrating = true;
    void import('./pointerDrag').then(
      async ({ createPointerDrag }) => {
        // A pending import resolving after unmount must not bind a dead board.
        if (destroyed) return;
        engine = createPointerDrag(ghost, { start: dragStart, over: dragOver, move: dragMove, end: dragEnd });
        await tick();
        const press = pending;
        pending = null;
        if (press && !destroyed && press.target instanceof Element && press.target.isConnected)
          press.target.dispatchEvent(new PointerEvent('pointerdown', press));
      },
      () => {
        hydrating = false;
      },
    );
  }
  $effect(() => {
    const idle = window.requestIdleCallback
      ? window.requestIdleCallback(hydrate, { timeout: 2500 })
      : window.setTimeout(hydrate, 1500);
    return () => (window.cancelIdleCallback ? window.cancelIdleCallback(idle) : window.clearTimeout(idle));
  });
  $effect(() => () => {
    destroyed = true;
    engine?.destroy();
  });
  // Not an interaction handler (nothing happens for the user); it only warms the engine.
  function warmUp(node: HTMLElement) {
    node.addEventListener('pointerover', hydrate, { once: true });
    node.addEventListener('pointerdown', hold);
    window.addEventListener('pointerup', release, true);
    window.addEventListener('pointercancel', release, true);
    const stop = () => {
      node.removeEventListener('pointerover', hydrate);
      node.removeEventListener('pointerdown', hold);
      window.removeEventListener('pointerup', release, true);
      window.removeEventListener('pointercancel', release, true);
    };
    return stop;
  }

  function dragStart(id: string) {
    if (saving) {
      say('Saving the last change…', true);
      return;
    }
    const run = week.runs.find((r) => r.id === id);
    if (!run) return;
    dragging = id;
    onhold(true);
    say(`Picked up ${runTitle(run)}, ${whenLabel(week, run.day, run.time)}.`, true);
  }

  // Where a pointer drop would land: on a card's middle band it swaps with
  // that card; elsewhere it moves between cards, at the time that gives
  // (dropTime.ts). The indicator line or the swap target shows which.
  interface MovePlan {
    kind: 'move';
    day: number;
    /** The card the indicator line sits on, and on which edge. */
    mark: { runId: string; edge: 'before' | 'after' } | null;
    drop: DropTime;
    clash: string | null;
  }
  interface SwapPlan {
    kind: 'swap';
    day: number;
    withId: string;
    text: string;
    clash: string | null;
  }
  type Plan = MovePlan | SwapPlan;
  let plan = $state<Plan | null>(null);
  let pointer = { x: 0, y: 0 };
  let board: HTMLDivElement;

  // Opening the pane narrows the board: keep the selected card in view, clear of the edge fades.
  $effect(() => {
    const id = selectedRun;
    if (!id || !board) return;
    const FADE = 48;
    const keep = () => {
      const card = board.querySelector<HTMLElement>(`[data-run="${CSS.escape(id)}"]`);
      if (!card || board.classList.contains('planner--dragging')) return;
      const view = board.getBoundingClientRect();
      const box = card.getBoundingClientRect();
      if (box.left < view.left + FADE) board.scrollLeft -= view.left + FADE - box.left;
      else if (box.right > view.right - FADE) board.scrollLeft += box.right - (view.right - FADE);
    };
    keep();
    const seen = new ResizeObserver(keep);
    seen.observe(board);
    return () => seen.disconnect();
  });

  function planFor(run: Run, day: number): Plan {
    // Cards of that day in board order, without the dragged one, and where the pointer sits among them.
    const cards = [...board.querySelectorAll<HTMLElement>(`[data-day="${day}"] [data-run]`)].filter((card) => card.dataset.run !== run.id);
    const runs = cards.map((card) => week.runs.find((r) => r.id === card.dataset.run));
    const zone = zoneAt(
      pointer.y,
      cards.map((card, i) => {
        const box = card.getBoundingClientRect();
        return { id: card.dataset.run!, top: box.top, height: box.height, swappable: !!runs[i] && ACTIVE(runs[i]) };
      }),
    );
    if (zone.kind === 'swap') {
      const other = runs.find((r) => r?.id === zone.id)!;
      return { kind: 'swap', day, withId: other.id, text: swapText(run, other), clash: swapClash(run, other) };
    }
    const dayRuns = runs.flatMap((other) => (other ? [timed(other)] : []));
    const drop = dropTime(timed(run), dayRuns, zone.index);
    const markCard = cards[zone.index] ?? cards[zone.index - 1];
    return {
      kind: 'move',
      day,
      mark: markCard ? { runId: markCard.dataset.run!, edge: cards[zone.index] ? 'before' : 'after' } : null,
      drop,
      clash: clashText(run, { day, time: drop.time }),
    };
  }

  const planKey = (p: Plan | null) => (p ? (p.kind === 'swap' ? `swap ${p.withId}` : `move ${p.day} ${p.drop.time}`) + ` ${p.clash}` : '');

  function replan() {
    const run = draggedRun;
    if (!run || overDay === null) return;
    const next = planFor(run, overDay);
    const changed = planKey(next) !== planKey(plan);
    plan = next;
    if (!changed) return;
    const clash = next.clash ? ` Clash: ${next.clash}.` : '';
    if (next.kind === 'swap') {
      say(`${next.text}.${clash}`, true);
    } else {
      const edge = next.drop.held ? ` (as ${next.drop.held === 'start' ? 'early' : 'late'} as the day goes)` : '';
      say(`Drop on ${describeSlot({ day: next.day, time: next.drop.time }, ctx)}${edge}.${clash}`, true);
    }
  }

  function dragOver(day: number | null) {
    if (day !== null && day !== overDay) {
      overDay = day;
      replan();
    }
  }

  function dragMove(x: number, y: number) {
    pointer = { x, y };
    replan();
  }

  function dragEnd(id: string, day: number | null, canceled: boolean) {
    if (dragging !== id) return;
    quietUntil = performance.now() + 400;
    const last = plan;
    dragging = null;
    overDay = null;
    plan = null;
    // Release the hold only after the move has started (see handleKey).
    const release = () => onhold(false);
    const run = week.runs.find((r) => r.id === id);
    if (!run) return release();
    if (!canceled && day !== null && last?.kind === 'swap' && last.day === day) {
      const other = week.runs.find((r) => r.id === last.withId);
      say(`${last.text}.${last.clash ? ` Clash: ${last.clash}; saved anyway.` : ''}`);
      if (other) commitSwap(run.id, other.id);
      return release();
    }
    const to = day === null ? null : { day, time: last?.kind === 'move' && last.day === day ? last.drop.time : run.time };
    if (canceled || to === null) {
      say(`Move cancelled. ${runTitle(run)} stays on ${whenLabel(week, run.day, run.time)}.`);
    } else if (to.day === run.day && to.time === run.time) {
      say(`Dropped ${runTitle(run)} where it was. Nothing changed.`);
    } else {
      const clash = clashText(run, to);
      say(`Dropped ${runTitle(run)} on ${describeSlot(to, ctx)}.${clash ? ` Clash: ${clash}; saved anyway.` : ''}`);
      commit(run.id, to);
    }
    release();
  }

  const drag = $derived(engine?.card ?? null);
  const drop = $derived(engine?.day ?? null);
</script>


<WeekRail days={week.days} runs={week.runs} />

<div class="board planner" data-fid="week-board" class:planner--dragging={dragging !== null} data-hydrated={engine ? "" : null} bind:this={board} {@attach warmUp} {@attach scrollEdges}>
  {#each week.days as day (day.index)}
    {@const runs = byDay[day.index] ?? []}
    <PlannerColumn
      {day}
      count={runs.length}
      targeted={(lift.kind === 'lifted' && lift.at.day === day.index) || overDay === day.index}
      preview={previewFor(day.index)}
      {drop}
    >
      {#if runs.length === 0}
        <p class="board__none"><span aria-hidden="true">·</span><span class="vh">Nothing on</span></p>
      {:else}
        <ul class="board__runs">
          {#each runs as run (run.id)}
            <PlannerCard
              {run}
              {week}
              {helpId}
              selected={run.id === selectedRun}
              lifted={lift.kind === 'lifted' && lift.runId === run.id}
              dragging={dragging === run.id}
              clash={cardClash.get(run.id) ?? null}
              dropMark={plan?.kind === 'move' && plan.mark?.runId === run.id ? plan.mark.edge : null}
              swapTarget={plan?.kind === 'swap' && plan.withId === run.id}
              onopen={open}
              onkey={handleKey}
              onblur={handleBlur}
              {drag}
              {onreread}
              {rereadOff}
              rereadBusy={busyChannels?.has(run.channel_id) ?? false}
            />
          {/each}
        </ul>
      {/if}
    </PlannerColumn>
  {/each}
</div>

<!-- The drop indicator rides with the pointer: the time the drop gives, and
  a clash when it double-books somebody (announced politely as it changes). -->
<div class="dnd-ghost runcard" class:dnd-ghost--on={draggedRun !== undefined} bind:this={ghost} aria-hidden="true">
  {#if draggedRun}
    {#if plan?.kind === 'swap'}
      <span class="dnd-ghost__swap">{plan.text}</span>
    {:else}
      <span class="runcard__time"
        >{#if plan && plan.drop.time !== draggedRun.time}<span class="dnd-ghost__from">{draggedRun.time ?? 'own time'}</span> → {plan.drop
            .time ?? 'own time'}{:else}{draggedRun.time ?? 'own time'}{/if}</span
      >
      <span>{runTitle(draggedRun)}</span>
      {#if plan?.drop.held}<span class="dnd-ghost__note">{plan.drop.held === 'start' ? 'earliest' : 'latest'} the day allows</span>{/if}
    {/if}
    {#if plan?.clash}<span class="plan-clash"><Icon name="alert-triangle" /> Clash: {plan.clash}</span>{/if}
  {/if}
</div>

<LiveRegion message={urgent} assertive />
<LiveRegion message={passing} />

<style>
  /* A clash: two overlapping runs that share a member. Icon and words in the
     risk text colour, on the card's own face (it still saves). */
  :global(.plan-clash) {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    width: fit-content;
    padding: 0 0.4rem;
    border: 1.5px solid var(--risk);
    border-radius: 999px;
    background: var(--surface);
    color: var(--risk-text);
    font-family: var(--body);
    font-size: var(--fs-mini);
    font-weight: 700;
    line-height: 1.5;
  }

  .planner--dragging {
    cursor: grabbing;
    user-select: none;
  }

  .dnd-ghost {
    position: fixed;
    top: 0;
    left: 0;
    z-index: 60;
    display: none;
    gap: 0.1rem;
    padding: 0.4rem 0.6rem;
    pointer-events: none;
    box-shadow: var(--shadow);
    border-left-color: var(--accent);
  }

  .dnd-ghost--on {
    display: grid;
    max-width: 18rem;
  }

  .dnd-ghost__from {
    color: var(--dim);
    text-decoration: line-through;
  }

  .dnd-ghost__swap {
    font-family: var(--mono);
    font-size: var(--fs-small);
    font-weight: 700;
  }

  .dnd-ghost__note {
    font-size: var(--fs-mini);
    color: var(--dim);
  }
</style>
