<!--
  A month as six boss-week rows (P_Dates, P_DatesSpec): columns start on the
  reset day, so a boss week is one row. Arrow keys move the roving focus; Enter
  and Space press the focused day (native button activation).
-->
<script lang="ts">
  import { dayOfMonth, longLabel, monthCells, monthIndex, monthOf, monthTitle, weekHeads, weekStartOf } from './calendar';

  let {
    y,
    m,
    firstDow,
    today,
    from,
    to,
    focus,
    onpick,
    onmove,
  }: {
    y: number;
    m: number;
    firstDow: number;
    today: number;
    /** Range start (or the one date); with `to`, the same day while an end is awaited. */
    from: number | null;
    to: number | null;
    focus: number;
    onpick: (day: number) => void;
    /** The keyboard moved the focus (clamped to today); the parent turns the month. */
    onmove: (day: number) => void;
  } = $props();

  let grid = $state<HTMLDivElement>();
  let moved = false;
  const heads = $derived(weekHeads(firstDow));
  const rows = $derived.by(() => {
    const cells = monthCells(y, m, firstDow);
    return Array.from({ length: 6 }, (_, r) => cells.slice(r * 7, r * 7 + 7));
  });
  // Month changes cross-fade: alternating animation names restart the fade.
  const phase = $derived(monthIndex(y, m) % 2 ? 'odd' : 'even');

  const STEPS: Record<string, (day: number) => number> = {
    ArrowLeft: (d) => d - 1,
    ArrowRight: (d) => d + 1,
    ArrowUp: (d) => d - 7,
    ArrowDown: (d) => d + 7,
    Home: (d) => weekStartOf(d, firstDow),
    End: (d) => weekStartOf(d, firstDow) + 6,
    PageUp: (d) => d - 28,
    PageDown: (d) => d + 28,
  };

  function key(event: KeyboardEvent) {
    const step = STEPS[event.key];
    if (!step) return;
    event.preventDefault();
    moved = true;
    onmove(Math.min(today, step(focus)));
  }

  $effect(() => {
    void focus;
    void rows;
    if (!moved) return;
    moved = false;
    grid?.querySelector<HTMLButtonElement>(`[data-day="${focus}"]`)?.focus({ preventScroll: true });
  });

  function name(day: number): string {
    const parts = [longLabel(day)];
    if (day === today) parts.push('today');
    if (from !== null && to !== null && day === from && day === to) parts.push('picked');
    else if (day === from) parts.push('range start');
    else if (day === to) parts.push('range end');
    if (day > today) parts.push('in the future');
    return parts.join(', ');
  }
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
<div class="dp-grid dp-grid--{phase}" data-fid="dr-grid" role="grid" aria-label={monthTitle(y, m)} bind:this={grid} onkeydown={key}>
  <div class="dp-grid__row" role="row">
    {#each heads as head, i (head.short)}
      <span class="dp-grid__head" class:dp-grid__head--reset={i === 0} role="columnheader" aria-label={head.long}>{head.short}</span>
    {/each}
  </div>
  {#each rows as row (row[0])}
    <div class="dp-grid__row" role="row">
      {#each row as day (day)}
        {@const start = day === from}
        {@const end = day === to}
        {@const inside = from !== null && to !== null && day > from && day < to}
        {@const future = day > today}
        <button
          type="button"
          class="dp-day"
          class:dp-day--out={monthOf(day).m !== m}
          class:dp-day--today={day === today}
          class:dp-day--start={start}
          class:dp-day--end={end}
          class:dp-day--in={inside}
          data-fid="dr-day"
          data-day={day}
          role="gridcell"
          aria-label={name(day)}
          aria-selected={start || end || inside}
          aria-disabled={future || undefined}
          tabindex={day === focus ? 0 : -1}
          onclick={() => {
            if (!future) onpick(day);
          }}>{dayOfMonth(day)}</button
        >
      {/each}
    </div>
  {/each}
</div>
