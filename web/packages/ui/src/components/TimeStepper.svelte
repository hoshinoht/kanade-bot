<!--
  One time as a spinbutton (board P_MoveStates): ↑/↓ and the chevrons step
  by `step` minutes, PgUp/PgDn by an hour, and it wraps at midnight. Without
  a value (an own-time run) it reads "--:--" and is disabled.
-->
<script lang="ts">
  let {
    value,
    step,
    label = 'Time',
    disabled = false,
    invalid = false,
    onchange,
    onenter,
    class: className = '',
  }: {
    /** Minutes after midnight, or null for no time. */
    value: number | null;
    step: number;
    label?: string;
    disabled?: boolean;
    /** The typed shortcut beside it holds an error. */
    invalid?: boolean;
    onchange: (minutes: number) => void;
    /** Enter on the time: the surrounding form's commit (Move). */
    onenter?: () => void;
    class?: string;
  } = $props();

  const DAY = 24 * 60;
  const off = $derived(disabled || value === null);
  const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  const text = $derived(value === null ? '--:--' : hhmm(value));

  function by(delta: number) {
    if (off || value === null) return;
    onchange((((value + delta) % DAY) + DAY) % DAY);
  }

  function keydown(event: KeyboardEvent) {
    const delta = { ArrowUp: step, ArrowDown: -step, PageUp: 60, PageDown: -60 }[event.key];
    if (delta !== undefined) {
      event.preventDefault();
      by(delta);
    } else if (event.key === 'Enter' && onenter) {
      event.preventDefault();
      onenter();
    }
  }
</script>

<div class="timestep {className}" class:timestep--off={off} class:timestep--bad={invalid} data-fid="move-time">
  <button type="button" class="timestep__btn" tabindex="-1" aria-label="{step} minutes earlier" disabled={off} onclick={() => by(-step)}
    ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M6 9l6 6 6-6" /></svg></button
  >
  <span
    class="timestep__value mono"
    role="spinbutton"
    tabindex={off ? -1 : 0}
    aria-label={label}
    aria-valuenow={value ?? 0}
    aria-valuemin={0}
    aria-valuemax={DAY - 1}
    aria-valuetext={value === null ? 'no time set' : text}
    aria-disabled={off ? 'true' : undefined}
    aria-invalid={invalid ? 'true' : undefined}
    onkeydown={keydown}>{text}</span
  >
  <button type="button" class="timestep__btn" tabindex="-1" aria-label="{step} minutes later" disabled={off} onclick={() => by(step)}
    ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M6 15l6-6 6 6" /></svg></button
  >
</div>
