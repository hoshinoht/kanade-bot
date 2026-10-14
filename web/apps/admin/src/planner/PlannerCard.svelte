<script lang="ts">
  import { untrack } from 'svelte';
  import type { Run, Week } from '@kanade/api-types';
  import { AnswerBar, answerCounts, answerWords, BossTag, Icon, openPlaces, RowContent, RunCardBody, runAccessibleName, runTitle, TWO_PANE_QUERY } from '@kanade/ui';
  import { PICK_KEY } from './keyboardMove';

  let {
    run,
    week,
    lifted,
    dragging,
    helpId,
    onopen,
    onkey,
    onblur,
    drag,
    onreread,
    rereadBusy,
    rereadOff = null,
    clash = null,
    dropMark = null,
    swapTarget = false,
    selected = false,
  }: {
    run: Run;
    week: Week;
    lifted: boolean;
    dragging: boolean;
    helpId: string;
    onopen: (run: Run) => void;
    onkey: (event: KeyboardEvent, run: Run) => void;
    onblur: (run: Run) => void;
    /** Pointer dragging, once the lazy engine has loaded; keyboard moves never wait for it. */
    drag?: ((card: HTMLElement, handle: HTMLElement | null, runId: string, movable: boolean) => () => void) | null;
    onreread?: (run: Run) => void;
    rereadBusy?: boolean;
    /** Why re-reading is off (the server's sentence) and the id of the note that shows it. */
    rereadOff?: { note: string; id: string } | null;
    /** Who it double-books, and where ("Asahi in HFA 21:00"); overlap alone is not a clash. */
    clash?: string | null;
    /** A pointer drag would land just before or after this card. */
    dropMark?: 'before' | 'after' | null;
    /** A pointer drop here would swap the two runs' slots. */
    swapTarget?: boolean;
    selected?: boolean;
  } = $props();

  const movable = $derived(run.status !== 'done' && run.status !== 'cancelled');
  const NOOP = () => {};
  // The attachment's identity follows the engine: cards mounted before it
  // hydrates bind when it arrives, not only if it won the race to mount.
  const dragAttach = $derived(
    drag ? (card: HTMLElement) => drag(card, card.querySelector<HTMLElement>('[data-handle]'), run.id, movable) : NOOP,
  );

  // Grow on hover (fine pointers) and on keyboard focus, never on click. The
  // full face grows in flow (the cards below move down rather than being
  // covered); a press, a lift or a drag settles it at once (plan-card--settle)
  // and a press keeps it quiet until the pointer leaves.
  let card = $state<HTMLLIElement>();
  let hovered = $state(false);
  let keyFocus = $state(false);
  let quiet = $state(false);
  let grown = $state(false);
  const HOVER = '(hover: hover) and (pointer: fine)';

  function canGrow(li: HTMLElement) {
    return matchMedia(TWO_PANE_QUERY).matches && !li.closest('.week-window--phone');
  }

  $effect(() => {
    const want = (hovered || keyFocus) && !quiet && !lifted && !dragging;
    const li = card;
    const was = untrack(() => grown);
    if (!li) return;
    if (want && !was && canGrow(li)) grown = true;
    else if (!want && was) grown = false;
  });
</script>

<!-- The whole card is the drag source (after a small movement, or a long
  press on touch), so a click or tap still opens the sheet. The grip only
  says so; M on the focused card is the keyboard move. -->
<li
  class="runcard runcard--{run.status} plan-card expandable-row"
  class:plan-card--selected={selected}
  class:plan-card--movable={movable}
  class:plan-card--reread={onreread !== undefined}
  class:plan-card--lifted={lifted || dragging}
  class:plan-card--drop-before={dropMark === 'before'}
  class:plan-card--drop-after={dropMark === 'after'}
  class:plan-card--swap-target={swapTarget}
  data-run={run.id}
  class:plan-card--grown={grown}
  class:plan-card--settle={quiet || lifted || dragging}
  data-fid="week-card"
  bind:this={card}
  {@attach dragAttach}
  onpointerenter={(event) => (hovered = event.pointerType === 'mouse' && matchMedia(HOVER).matches)}
  onpointerleave={() => (hovered = quiet = false)}
  onpointerdown={() => (quiet = true)}
>
  <button
    type="button"
    class="plan-card__open"
    data-handle={run.id}
    aria-current={selected ? 'true' : undefined}
    aria-label="{run.time ?? 'own time'} {runTitle(run)}: {runAccessibleName(week, run)}. Answers: {answerWords(answerCounts(run.participants))}.{clash ? ` Clash: ${clash}.` : ''} Open details"
    aria-describedby={movable ? helpId : undefined}
    aria-keyshortcuts={movable ? PICK_KEY : undefined}
    onclick={() => {
      quiet = true;
      onopen(run);
    }}
    onkeydown={movable ? (event) => onkey(event, run) : undefined}
    onfocus={(event) => (keyFocus = event.currentTarget.matches(':focus-visible'))}
    onblur={() => {
      keyFocus = false;
      // A keyboard open leaves no pointer behind; a pointer press stays quiet until it leaves.
      if (!hovered) quiet = false;
      onblur(run);
    }}
  >
    <span class="runcard__visual" aria-hidden="true"><RunCardBody {run} places={openPlaces(run)}>
      {#snippet bosses()}
        <!-- The full face shows on hover and keyboard focus only (CSS, plan-card--grown); selection keeps the ring. -->
        <RowContent expanded={false}>
          {#snippet compact()}<span class="plan-card__summary">{#each run.bosses as boss (boss.token)}<BossTag {boss} short />{/each}</span
            ><span class="plan-card__party plan-card__party--line">{run.participants.map((person) => person.name).join(' · ')}</span>{/snippet}
          <span class="runcard__bosses">{#each run.bosses as boss (boss.token)}<BossTag {boss} short={!grown} portrait={grown} />{/each}</span>
          <span class="plan-card__party">{run.participants.map((person) => person.name).join(' · ')}</span>
          <span class="plan-card__channel">{run.channel}</span>
        </RowContent>
      {/snippet}
    </RunCardBody><AnswerBar participants={run.participants} class="plan-card__answers" /></span>
    {#if clash}
      <!-- Icon and words, never colour alone; the full text is in the accessible name and the tooltip. -->
      <span class="plan-clash plan-card__clash" aria-hidden="true" title="Clash: {clash}"><Icon name="alert-triangle" /> Clash</span>
    {/if}
    {#if movable}
      <svg class="plan-card__grip" viewBox="0 0 6 10" aria-hidden="true" focusable="false">
        <circle cx="1" cy="1" r="1" /><circle cx="5" cy="1" r="1" />
        <circle cx="1" cy="5" r="1" /><circle cx="5" cy="5" r="1" />
        <circle cx="1" cy="9" r="1" /><circle cx="5" cy="9" r="1" />
      </svg>
    {/if}
  </button>
  {#if onreread}
    <!-- v4 days.html: the phone day list re-reads a run's channel from its row. -->
    <button
      type="button"
      class="plan-card__reread"
      aria-label="Re-read {run.channel} for {runTitle(run)}"
      aria-disabled={rereadBusy || rereadOff !== null}
      aria-describedby={rereadOff?.id}
      onclick={() => {
        if (!rereadBusy && !rereadOff) onreread(run);
      }}
    >
      <Icon name="refresh-cw" />
    </button>
  {/if}
</li>

<style>
  .plan-card {
    display: flex;
    align-items: stretch;
    padding: 0;
  }

  .plan-card--selected, .plan-card--selected:hover {
    --card-face: var(--select);
    border-radius: 18px;
    box-shadow: inset 0 0 0 1.5px var(--select-edge);
  }

  .plan-card__summary { font-size: var(--fs-small); font-weight: 600; }
  .plan-card__party, .plan-card__channel { display: block; margin-top: 4px; color: var(--dim-text); font-size: var(--fs-mini); }
  .plan-card__channel { font-family: var(--mono); }

  .plan-card__open {
    flex: 1 1 auto;
    display: grid;
    gap: 0.15rem;
    min-width: 0;
    overflow: hidden;
    padding: 0.4rem 0.45rem;
    border: 0;
    border-radius: calc(var(--r-sm) - 2px);
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .plan-card:hover {
    --card-face: var(--raise);
  }

  /* The grip owns a grid column beside the bosses, so no boss count or name
     length can run under it; the clock and tally span the full width above.
     On the card's own face so --dim holds its measured contrast over the
     entry-art veil. */
  .plan-card--movable .plan-card__open {
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: 0.3rem;
  }

  .plan-card--movable :global(.runcard__top) {
    grid-column: 1 / -1;
  }

  /* The answers bar runs under the bosses, the card's full width. */
  .plan-card :global(.plan-card__answers) {
    grid-column: 1 / -1;
    margin-top: 0.2rem;
  }

  .plan-card__grip {
    grid-column: 2;
    grid-row: 2;
    align-self: end;
    margin: 0 -0.15rem -0.1rem 0;
    width: 0.55rem;
    height: 0.8rem;
    padding: 0.1rem;
    box-sizing: content-box;
    border-radius: 3px;
    background: var(--card-face);
    fill: var(--dim);
    pointer-events: none;
    transition: transform 0.12s ease-out;
  }

  /* A token too long for its column drops its pill under the name rather than
     losing it past the card's edge. */
  .plan-card :global(.runcard__bosses .boss) {
    flex-wrap: wrap;
    row-gap: 0.1rem;
  }

  .plan-card--movable .plan-card__open {
    /* Long-press drags on touch: no callout or text selection in the way.
       Panning stays with the browser until the press activates. */
    -webkit-touch-callout: none;
    user-select: none;
  }

  /* Hover and keyboard focus say "this moves": grab cursor, a slight lift,
     the grip at full emphasis. Transform and shadow only. */
  .plan-card--movable {
    transition:
      transform 0.12s ease-out,
      box-shadow 0.12s ease-out,
      border-radius 220ms ease;
  }

  @media (hover: hover) and (pointer: fine) {
    .plan-card--movable .plan-card__open {
      cursor: grab;
    }

    .plan-card--movable:hover {
      transform: translateY(-1px);
      box-shadow: var(--shadow);
    }

    .plan-card--movable:hover .plan-card__grip {
      fill: var(--accent-text);
      transform: scale(1.2);
    }
  }

  .plan-card--movable:has(.plan-card__open:focus-visible) {
    transform: translateY(-1px);
    box-shadow: var(--shadow);
  }

  .plan-card--movable:has(.plan-card__open:focus-visible) .plan-card__grip {
    fill: var(--accent-text);
    transform: scale(1.2);
  }

  .plan-card__reread {
    display: none;
  }

  /* $single-pane in @kanade/ui _breakpoints.scss (plain CSS here). */
  @media (width < 900px) {
    .plan-card__reread {
      flex: none;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 2.5rem;
      min-height: 2.75rem;
      border: 0;
      border-left: 2px solid var(--line-soft);
      background: transparent;
      color: var(--dim);
      cursor: pointer;
    }

    .plan-card__reread:hover:not([aria-disabled='true']) {
      color: var(--accent);
      background: var(--raise);
    }

    .plan-card__reread[aria-disabled='true'] {
      cursor: not-allowed;
      opacity: 0.5;
    }
  }

  .plan-card--lifted {
    outline: 2px dashed var(--accent);
    outline-offset: 2px;
  }

  .plan-card__clash {
    grid-column: 1;
    justify-self: start;
  }

  /* Where a pointer drop lands: a bar on the card's top or bottom edge
     (box-shadow, so nothing on the board moves while dragging). */
  .plan-card--drop-before {
    box-shadow: 0 -4px 0 -1px var(--accent);
  }

  .plan-card--drop-after {
    box-shadow: 0 4px 0 -1px var(--accent);
  }

  /* The card a drop would swap with: a solid accent ring and the select
     container (the ghost says "Swap with …" in words). */
  .plan-card--swap-target {
    outline: 3px solid var(--accent);
    outline-offset: 1px;
    --card-face: var(--select);
  }
</style>
