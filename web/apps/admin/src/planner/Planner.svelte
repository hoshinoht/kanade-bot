<script lang="ts">
  import { tick } from 'svelte';
  import type { Run, Week } from '@kanade/api-types';
  import { LiveRegion, WeekRail, dayLabel, runTitle, sortRuns, whenLabel } from '@kanade/ui';
  import { IDLE, cancel, describeSlot, onKey, type LiftState, type MovableRun, type Slot } from './keyboardMove';
  import PlannerCard from './PlannerCard.svelte';
  import PlannerColumn from './PlannerColumn.svelte';
  import type { PointerDrag } from './pointerDrag';

  let {
    week,
    helpId,
    onmove,
    onopen,
    onhold,
    onreread,
    busyChannels,
  }: {
    week: Week;
    /** The week header's move instructions: every movable card is described by them. */
    helpId: string;
    onmove: (runId: string, to: Slot) => void;
    onopen: (run: Run) => void;
    onhold: (holding: boolean) => void;
    onreread?: (run: Run) => void;
    busyChannels?: Set<string>;
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

  const ctx = $derived({ dayLabel: (d: number) => dayLabel(week, d), lastDay: week.days.length - 1 });
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
    return { id: run.id, day: run.day, time: run.time, label: runTitle(run) };
  }

  async function focusHandle(runId: string) {
    await tick();
    document.querySelector<HTMLElement>(`[data-handle="${CSS.escape(runId)}"]`)?.focus();
  }

  function commit(runId: string, to: Slot) {
    onmove(runId, to);
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
    const outcome = onKey(lift, event.key, movable(run), ctx);
    if (!outcome.handled) return;
    event.preventDefault();
    quietUntil = performance.now() + 400;
    lift = outcome.state;
    onhold(lift.kind === 'lifted');
    if (outcome.announce) say(outcome.announce);
    if (outcome.commit) commit(outcome.commit.runId, outcome.commit.to);
  }

  function handleBlur(run: Run) {
    if (lift.kind !== 'lifted' || lift.runId !== run.id) return;
    const outcome = cancel(lift, movable(run), ctx);
    lift = outcome.state;
    onhold(false);
    if (outcome.announce) say(outcome.announce);
  }

  function previewFor(day: number): string | null {
    const current = lift;
    if (current.kind === 'lifted' && current.at.day === day) {
      const run = week.runs.find((r) => r.id === current.runId);
      return run ? `${current.at.time ?? 'own time'} ${runTitle(run)}` : null;
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
        engine = createPointerDrag(ghost, { start: dragStart, over: dragOver, end: dragEnd });
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
    const run = week.runs.find((r) => r.id === id);
    if (!run) return;
    dragging = id;
    onhold(true);
    say(`Picked up ${runTitle(run)}, ${whenLabel(week, run.day, run.time)}.`, true);
  }

  function dragOver(day: number | null) {
    if (day !== null && day !== overDay) {
      overDay = day;
      say(`Over ${dayLabel(week, day)}.`, true);
    }
  }

  function dragEnd(id: string, day: number | null, canceled: boolean) {
    quietUntil = performance.now() + 400;
    dragging = null;
    overDay = null;
    onhold(false);
    const run = week.runs.find((r) => r.id === id);
    if (!run) return;
    if (canceled || day === null) {
      say(`Move cancelled. ${runTitle(run)} stays on ${whenLabel(week, run.day, run.time)}.`);
    } else if (day === run.day) {
      say(`Dropped ${runTitle(run)} where it was. Nothing changed.`);
    } else {
      const to = { day, time: run.time };
      say(`Dropped ${runTitle(run)} on ${describeSlot(to, ctx)}.`);
      commit(run.id, to);
    }
  }

  const drag = $derived(engine?.card ?? null);
  const drop = $derived(engine?.day ?? null);
</script>


<WeekRail days={week.days} runs={week.runs} />

<div class="board planner" class:planner--dragging={dragging !== null} data-hydrated={engine ? "" : null} {@attach warmUp}>
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
              lifted={lift.kind === 'lifted' && lift.runId === run.id}
              dragging={dragging === run.id}
              onopen={open}
              onkey={handleKey}
              onblur={handleBlur}
              {drag}
              {onreread}
              rereadBusy={busyChannels?.has(run.channel_id) ?? false}
            />
          {/each}
        </ul>
      {/if}
    </PlannerColumn>
  {/each}
</div>

<div class="dnd-ghost runcard" class:dnd-ghost--on={draggedRun !== undefined} bind:this={ghost} aria-hidden="true">
  {#if draggedRun}
    <span class="runcard__time">{draggedRun.time ?? 'own time'}</span>
    <span>{runTitle(draggedRun)}</span>
  {/if}
</div>

<LiveRegion message={urgent} assertive />
<LiveRegion message={passing} />

<style>
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
  }
</style>
