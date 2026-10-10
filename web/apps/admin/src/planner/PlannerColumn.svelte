<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { WeekDay } from '@kanade/api-types';
  import { DayColumn, Icon } from '@kanade/ui';

  let {
    day,
    count,
    targeted,
    preview,
    drop,
    children,
  }: {
    day: WeekDay;
    count: number;
    targeted: boolean;
    /** The keyboard lift's drop indicator: where it would land, and any clash. */
    preview: { text: string; clash: string | null } | null;
    drop?: ((column: HTMLElement, day: number) => (() => void) | void) | null;
    children: Snippet;
  } = $props();

  const NOOP = () => {};
  // As on the cards: columns mounted before hydration bind when it arrives.
  const dropAttach = $derived(drop ? (column: HTMLElement) => drop(column, day.index) : NOOP);
</script>

<DayColumn {day} {count} extraClass={targeted ? 'board__col--target' : ''} attach={dropAttach}>
  {#if preview}
    <p class="plan-preview" aria-hidden="true">
      Drop here · {preview.text}
      {#if preview.clash}<span class="plan-clash"><Icon name="alert-triangle" /> Clash: {preview.clash}</span>{/if}
    </p>
  {/if}
  {@render children()}
</DayColumn>

<style>
  :global(.board__col--target) {
    border-color: var(--accent);
    border-style: solid;
    background: var(--accent-wash);
  }

  /* $two-pane in @kanade/ui _breakpoints.scss (plain CSS here). */
  @media (width >= 900px) {
    :global(.board__col--target.board__col--empty) {
      flex: 1 1 0;
    }
  }

  .plan-preview {
    display: grid;
    gap: 0.15rem;
    margin: 0.4rem 0.4rem 0;
    padding: 0.35rem 0.5rem;
    border: 2px dashed var(--accent);
    border-radius: var(--r-sm);
    font-family: var(--mono);
    font-size: var(--fs-small);
    color: var(--ink);
    background: var(--surface);
  }
</style>
