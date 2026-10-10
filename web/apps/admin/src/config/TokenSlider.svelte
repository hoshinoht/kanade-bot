<!--
  A token count as a range slider paired with an exact number field: the
  slider moves between round stops (arrows step, Home/End jump), the field
  takes any whole value. Local windows mark 16k on the track and, past it,
  switch to the warning tone with the server's words (never blocking).
-->
<script lang="ts">
  import { Icon } from '@kanade/ui';
  import { LOCAL_WARNING, LOCAL_WARNING_TOKENS, stopIndex, tokens, tokenStops } from './context';

  let {
    label,
    value = $bindable(),
    max,
    local = false,
    disabled = false,
  }: { label: string; value: number | null; max: number; local?: boolean; disabled?: boolean } = $props();

  const uid = $props.id();
  const stops = $derived(tokenStops(max));
  const position = $derived(stopIndex(stops, value));
  const warn = $derived(local && value !== null && value > LOCAL_WARNING_TOKENS);
  const mark = $derived(local && max > LOCAL_WARNING_TOKENS ? stops.indexOf(LOCAL_WARNING_TOKENS) / (stops.length - 1) : null);

  // CSP: no inline style attributes, so the marker's place is set through CSSOM.
  function place(node: HTMLElement) {
    if (mark !== null) node.style.setProperty('--mark', String(mark));
  }
</script>

<div class="token" class:token--warn={warn}>
  <label class="token__label" for="{uid}-value">{label}</label>
  <div class="token__controls">
    <div class="token__track">
      <input
        type="range"
        min="0"
        max={stops.length - 1}
        step="1"
        value={position}
        {disabled}
        aria-label={label}
        aria-valuetext={value === null ? 'not set' : `${tokens(value)} tokens`}
        aria-describedby={warn ? `${uid}-warn` : undefined}
        oninput={(event) => (value = stops[Number(event.currentTarget.value)] ?? max)}
      />
      {#if mark !== null}
        <span class="token__mark" aria-hidden="true" {@attach place}>16k</span>
      {/if}
    </div>
    <span class="token__exact">
      <input
        id="{uid}-value"
        class="token__value"
        type="number"
        min="1"
        {max}
        step="1"
        required
        inputmode="numeric"
        {disabled}
        bind:value
        aria-describedby={warn ? `${uid}-warn` : undefined}
      />
      <span class="token__unit">tokens</span>
    </span>
  </div>
  <div aria-live="polite">
    {#if warn}
      <p class="settings__warn" id="{uid}-warn"><Icon name="alert-triangle" /><span>{LOCAL_WARNING}</span></p>
    {/if}
  </div>
</div>

<style>
  .token {
    display: grid;
    gap: 0.2rem;
    margin: 0.55rem 0 0;
  }

  .token__label {
    font-size: var(--fs-body-sm);
    font-weight: 600;
  }

  .token__controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.35rem 0.6rem;
  }

  .token__track {
    position: relative;
    flex: 1 1 10rem;
    min-width: 8rem;
    max-width: 28rem;
    padding-bottom: 0.95rem;
  }

  .token__track input {
    display: block;
    width: 100%;
    min-height: 1.75rem;
    margin: 0;
    padding: 0;
    border: 0;
    background: transparent;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .token__track input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* The thumb's centre travels 8px in from each edge (Chromium's 16px thumb). */
  .token__mark {
    position: absolute;
    bottom: 0;
    left: calc(var(--mark, 0) * (100% - 16px) + 8px);
    transform: translateX(-50%);
    padding-top: 0.15rem;
    font-size: var(--fs-micro);
    font-family: var(--mono);
    color: var(--dim-text);
    line-height: 1;
  }

  .token__mark::before {
    content: '';
    position: absolute;
    left: 50%;
    bottom: 100%;
    width: 2px;
    height: 0.45rem;
    background: var(--warn);
  }

  /* The field and its unit wrap together, never apart. */
  .token__exact {
    display: inline-flex;
    flex: none;
    align-items: center;
    gap: 0.4rem;
  }

  .token__value {
    width: 6.5rem;
    text-align: right;
    font-family: var(--mono);
  }

  .token__unit {
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  /* Past 16k on a local model: the warning tone, plus the words below. */
  .token--warn .token__track input {
    accent-color: var(--warn);
  }

  .token--warn .token__value {
    border-color: var(--warn);
  }
</style>
