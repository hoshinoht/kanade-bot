<!--
  v4 partials/rail.html: the boss week's shape on a phone — seven cells from
  the reset day, one pip per run. A cell scrolls its day into view inside
  the panel that owns the scrolling (never the frame). Wide screens hide it:
  the board is the rail, grown up.
-->
<script lang="ts">
  import type { RunStatus, WeekDay } from '@kanade/api-types';
  import { dayNumber } from '../format';

  let { days, runs }: { days: WeekDay[]; runs: { day: number; status: RunStatus }[] } = $props();
  const byDay = $derived(days.map((d) => runs.filter((r) => r.day === d.index)));

  function scrollOwner(node: HTMLElement): HTMLElement | null {
    for (let el = node.parentElement; el; el = el.parentElement) {
      const y = getComputedStyle(el).overflowY;
      if ((y === 'auto' || y === 'scroll') && el.scrollHeight > el.clientHeight) return el;
    }
    return null;
  }

  function show(event: MouseEvent, index: number) {
    const rail = event.currentTarget instanceof HTMLElement ? event.currentTarget.closest<HTMLElement>('.rail') : null;
    const panel = rail?.parentElement;
    const col = panel?.querySelector<HTMLElement>(`[data-day="${index}"]`);
    if (!rail || !col) return;
    const owner = scrollOwner(col);
    if (owner) owner.scrollTop += col.getBoundingClientRect().top - owner.getBoundingClientRect().top - rail.offsetHeight - 6;
    col.querySelector<HTMLElement>('.board__head')?.focus({ preventScroll: true });
  }
</script>

<nav class="rail" aria-label="Days of this boss week">
  {#each days as day, i (day.index)}
    {@const count = byDay[i]?.length ?? 0}
    <button
      type="button"
      class="rail__day"
      class:rail__day--today={day.is_today}
      class:rail__day--reset={day.is_reset}
      class:rail__day--empty={count === 0}
      aria-current={day.is_today ? 'date' : undefined}
      aria-label="{day.dow} {dayNumber(day.date)}, {count} run{count === 1 ? '' : 's'}{day.is_reset ? ', the boss week starts' : ''}"
      onclick={(event) => show(event, day.index)}
    >
      <span class="rail__dow" aria-hidden="true">{day.dow}</span>
      <span class="rail__num" aria-hidden="true">{dayNumber(day.date)}</span>
      <span class="rail__pips" aria-hidden="true">
        {#each byDay[i] ?? [] as run, n (n)}<span class="pip-dot pip-dot--{run.status}"></span>{/each}
      </span>
    </button>
  {/each}
</nav>
