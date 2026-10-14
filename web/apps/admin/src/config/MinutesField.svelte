<!--
  Minutes as a range slider paired with an exact number field (the Context
  windows pattern, TokenSlider): the slider moves in 5-minute stops, the
  field takes any whole value in range.
-->
<script lang="ts">
  let {
    label,
    value = $bindable(),
    min,
    max,
    invalid = false,
  }: { label: string; value: number | null; min: number; max: number; invalid?: boolean } = $props();

  const uid = $props.id();
</script>

<div class="minutes">
  <label class="minutes__label" for="{uid}-value">{label}</label>
  <div class="minutes__controls">
    <input
      class="minutes__track"
      type="range"
      {min}
      {max}
      step="5"
      value={value ?? min}
      aria-label={label}
      aria-valuetext={value === null ? 'not set' : `${value} minutes`}
      oninput={(event) => (value = Number(event.currentTarget.value))}
    />
    <span class="minutes__exact">
      <input
        id="{uid}-value"
        class="minutes__value"
        type="number"
        {min}
        {max}
        step="1"
        required
        inputmode="numeric"
        bind:value
        aria-invalid={invalid}
      />
      <span class="minutes__unit">minutes</span>
    </span>
  </div>
</div>

<style>
  .minutes {
    display: grid;
    gap: 0.2rem;
    margin: 0.55rem 0 0;
  }

  .minutes__label {
    font-size: var(--fs-body-sm);
    font-weight: 600;
  }

  .minutes__controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem 0.6rem;
  }

  .minutes__track {
    flex: 1 1 10rem;
    min-width: 8rem;
    max-width: 28rem;
    min-height: 1.75rem;
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .minutes__track:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* The field and its unit wrap together, never apart. */
  .minutes__exact {
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: 0.4rem;
  }

  .minutes__value {
    width: 5.5rem;
    text-align: right;
    font-family: var(--mono);
  }

  .minutes__unit {
    color: var(--dim-text);
    font-size: var(--fs-small);
  }
</style>
