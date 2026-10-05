<!--
  One time as a spinbutton (board P_MoveStates): ↑/↓ and the chevrons step
  by `step` minutes, PgUp/PgDn by an hour, and it wraps at midnight. Without
  a value (an own-time run) it reads "--:--" and is disabled, unless `start`
  says where the first step lands. With `ontype` the value is a text field
  that also takes a typed time (the Fixed editor); Enter submits its form.
-->
<script lang="ts">
  let {
    value,
    step,
    label = 'Time',
    disabled = false,
    invalid = false,
    start,
    draft,
    placeholder,
    id,
    onchange,
    onenter,
    ontype,
    class: className = '',
  }: {
    /** Minutes after midnight, or null for no time. */
    value: number | null;
    step: number;
    label?: string;
    disabled?: boolean;
    /** The typed shortcut beside it holds an error. */
    invalid?: boolean;
    /** Minutes the first step lands on while there is no value (stays enabled). */
    start?: number;
    /** The typed text shown in the field (`ontype` only), kept as typed. */
    draft?: string;
    placeholder?: string;
    /** The typed field's id (`ontype` only), for a visible `<label for>`. */
    id?: string;
    onchange: (minutes: number) => void;
    /** Enter on the time: the surrounding form's commit (Move). */
    onenter?: () => void;
    /** Makes the value a text field; every keystroke arrives here. */
    ontype?: (text: string) => void;
    class?: string;
  } = $props();

  const DAY = 24 * 60;
  const off = $derived(disabled || (value === null && start === undefined));
  const hhmm = (m: number) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  const text = $derived(value === null ? '--:--' : hhmm(value));

  function by(delta: number) {
    if (off) return;
    if (value === null) onchange(start!);
    else onchange((((value + delta) % DAY) + DAY) % DAY);
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

<div class="timestep {className}" class:timestep--off={off} class:timestep--bad={invalid} class:timestep--typed={Boolean(ontype)} data-fid="move-time">
  <button type="button" class="timestep__btn" tabindex="-1" aria-label="{step} minutes earlier" disabled={off} onclick={() => by(-step)}
    ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M6 9l6 6 6-6" /></svg></button
  >
  {#if ontype}
    <input
      {id}
      class="timestep__value mono"
      role="spinbutton"
      aria-label={label}
      aria-valuenow={value ?? undefined}
      aria-valuemin={0}
      aria-valuemax={DAY - 1}
      aria-valuetext={value === null ? draft || 'no time set' : text}
      aria-invalid={invalid ? 'true' : undefined}
      value={draft ?? (value === null ? '' : text)}
      {placeholder}
      {disabled}
      size="5"
      autocomplete="off"
      spellcheck="false"
      oninput={(event) => ontype(event.currentTarget.value)}
      onkeydown={keydown}
    />
  {:else}
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
  {/if}
  <button type="button" class="timestep__btn" tabindex="-1" aria-label="{step} minutes later" disabled={off} onclick={() => by(step)}
    ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"><path d="M6 15l6-6 6 6" /></svg></button
  >
</div>
