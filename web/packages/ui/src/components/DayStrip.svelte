<!--
  The boss week's days as one connected radio group (board P_MoveStates):
  selected = accent fill and a full pill, today = an accent ring, the run's
  own day = a dashed ring, past days dashed, struck and aria-disabled, and up
  to three dots for other runs. `compact` is the weekday-only strip (the
  Fixed editor). ←/→ skip past days; Home/End go to `home`/the last open day.
-->
<script lang="ts" module>
  export interface StripDay {
    value: number;
    dow: string;
    /** Two-digit date; omitted on the weekday-only strip. */
    date?: string;
    tag?: string;
    dots?: number;
    past?: boolean;
    today?: boolean;
    reset?: boolean;
    from?: boolean;
    /** The accessible name: the date and every mark the cell shows. */
    label: string;
  }
</script>

<script lang="ts">
  let {
    days,
    value,
    label,
    compact = false,
    home,
    onpick,
    onenter,
    class: className = '',
  }: {
    days: StripDay[];
    value: number | null;
    label: string;
    compact?: boolean;
    /** Home's target (today); the first open day when absent. */
    home?: number;
    onpick: (value: number) => void;
    /** Enter on a day: the surrounding form's commit (Move). */
    onenter?: () => void;
    class?: string;
  } = $props();

  const buttons: HTMLButtonElement[] = $state([]);
  const current = $derived(Math.max(0, days.findIndex((d) => d.value === value)));

  function open(from: number, direction: -1 | 1): number {
    for (let i = from + direction; i >= 0 && i < days.length; i += direction) if (!days[i]!.past) return i;
    return from;
  }

  function go(index: number) {
    const day = days[index];
    if (!day || day.past) return;
    onpick(day.value);
    buttons[index]?.focus();
  }

  function keydown(event: KeyboardEvent, index: number) {
    const homeIndex = home === undefined ? open(-1, 1) : days.findIndex((d) => d.value === home);
    const target = {
      ArrowRight: open(index, 1),
      ArrowDown: open(index, 1),
      ArrowLeft: open(index, -1),
      ArrowUp: open(index, -1),
      Home: homeIndex,
      End: open(days.length, -1),
    }[event.key];
    if (target !== undefined) {
      event.preventDefault();
      go(target);
    } else if (event.key === 'Enter' && onenter) {
      event.preventDefault();
      onenter();
    }
  }
</script>

<div class="daystrip {className}" class:daystrip--compact={compact} role="radiogroup" aria-label={label} data-fid="move-days">
  {#each days as day, index (day.value)}
    <button
      type="button"
      role="radio"
      class="daystrip__day"
      class:daystrip__day--on={day.value === value}
      class:daystrip__day--today={day.today}
      class:daystrip__day--from={day.from}
      class:daystrip__day--reset={day.reset}
      class:daystrip__day--past={day.past}
      aria-checked={day.value === value}
      aria-disabled={day.past ? 'true' : undefined}
      aria-label={day.label}
      tabindex={index === current ? 0 : -1}
      data-fid="move-day"
      bind:this={buttons[index]}
      onclick={() => {
        if (!day.past) onpick(day.value);
      }}
      onkeydown={(event) => keydown(event, index)}
    >
      <span class="daystrip__dow">{day.dow}</span>
      {#if !compact}
        <span class="daystrip__date">{day.date}</span>
        <span class="daystrip__tag" aria-hidden="true"
          >{day.tag ?? ''}{#each { length: day.tag ? 0 : (day.dots ?? 0) }, n (n)}<i></i>{/each}</span
        >
      {/if}
    </button>
  {/each}
</div>
