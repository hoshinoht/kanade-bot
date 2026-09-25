<script lang="ts">
  import type { Run, Week } from '@kanade/api-types';
  import { Icon, RunCardBody, runAccessibleName, runTitle } from '@kanade/ui';
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
  } = $props();

  const movable = $derived(run.status !== 'done' && run.status !== 'cancelled');
  const NOOP = () => {};
  // The attachment's identity follows the engine: cards mounted before it
  // hydrates bind when it arrives, not only if it won the race to mount.
  const dragAttach = $derived(
    drag ? (card: HTMLElement) => drag(card, card.querySelector<HTMLElement>('[data-handle]'), run.id, movable) : NOOP,
  );
</script>

<!-- The whole card is the drag source (after a small movement, or a long
  press on touch), so a click or tap still opens the sheet. The grip only
  says so; M on the focused card is the keyboard move. -->
<li
  class="runcard runcard--{run.status} plan-card"
  class:plan-card--movable={movable}
  class:plan-card--reread={onreread !== undefined}
  class:plan-card--lifted={lifted || dragging}
  data-run={run.id}
  {@attach dragAttach}
>
  <button
    type="button"
    class="plan-card__open"
    data-handle={run.id}
    aria-label="{run.time ?? 'own time'} {runTitle(run)}: {runAccessibleName(week, run)}. Open details"
    aria-describedby={movable ? helpId : undefined}
    aria-keyshortcuts={movable ? PICK_KEY : undefined}
    onclick={() => onopen(run)}
    onkeydown={movable ? (event) => onkey(event, run) : undefined}
    onblur={() => onblur(run)}
  >
    <span class="runcard__visual" aria-hidden="true"><RunCardBody {run} /></span>
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
      aria-disabled={rereadBusy}
      onclick={() => {
        if (!rereadBusy) onreread(run);
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

  /* Tucked into the bottom-right corner, clear of the clock and tally above
     and of the bosses, which keep a gutter for it. On the card's own face so
     --dim holds its measured contrast over the entry-art veil. */
  .plan-card__grip {
    position: absolute;
    right: 0.3rem;
    bottom: 0.3rem;
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

  .plan-card--movable :global(.runcard__bosses) {
    padding-right: 0.9rem;
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
      box-shadow 0.12s ease-out;
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

  @media (max-width: 899px) {
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

    .plan-card__reread:hover {
      color: var(--accent);
      background: var(--raise);
    }

    /* The grip stays inside the open area, left of the re-read column. */
    .plan-card--reread .plan-card__grip {
      right: 2.8rem;
    }
  }

  .plan-card--lifted {
    outline: 2px dashed var(--accent);
    outline-offset: 2px;
  }
</style>
