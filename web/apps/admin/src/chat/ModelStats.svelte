<!--
  Chat's per-model summary in the page line (M3E spec "Chat"): compact chips
  for the two busiest models ("model 423 ✓ · p50 2.2 s"; fewer when the line
  is narrow) and a "+n models · e errors" button that opens the full table
  the old head card printed. A disclosure, not a modal: Escape or a click
  elsewhere closes it, and Escape returns focus to the button.
-->
<script lang="ts">
  import type { ChatSummary } from '@kanade/api-types';
  import { duration } from '../logs/format';

  let { summary }: { summary: ChatSummary[] } = $props();

  const uid = $props.id();
  const CHIPS = 2;
  const sorted = $derived([...summary].sort((a, b) => b.count - a.count || a.model.localeCompare(b.model)));
  const more = $derived(Math.max(0, sorted.length - CHIPS));
  const errors = $derived(summary.reduce((sum, m) => sum + m.errors, 0));
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
  const label = $derived(`${more ? `+${plural(more, 'model')}` : 'Per model'} · ${plural(errors, 'error')}`);

  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();

  function close(refocus: boolean) {
    open = false;
    if (refocus) button?.focus();
  }

  // Escape closes it from anywhere and gives focus back to the button; a
  // click or focus anywhere else closes it too.
  $effect(() => {
    if (!open) return;
    const onkeydown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      close(true);
    };
    document.addEventListener('keydown', onkeydown);
    const away = (event: Event) => {
      const target = event.target;
      if (target instanceof Node && !panel?.contains(target) && !button?.contains(target)) close(false);
    };
    document.addEventListener('pointerdown', away);
    document.addEventListener('focusin', away);
    return () => {
      document.removeEventListener('keydown', onkeydown);
      document.removeEventListener('pointerdown', away);
      document.removeEventListener('focusin', away);
    };
  });
</script>

<div class="modelstats">
  <ul class="modelstats__chips" aria-label="Busiest models, for these rows">
    {#each sorted.slice(0, CHIPS) as m (m.model)}
      <li class="modelstats__chip">
        <span class="modelstats__name mono">{m.model}</span>
        <span class="mono">{m.answered}<span aria-hidden="true">&nbsp;✓</span><span class="vh">&nbsp;answered</span> · p50 {duration(m.p50_ms)}</span>
      </li>
    {/each}
  </ul>
  <div class="modelstats__more">
    <button
      bind:this={button}
      type="button"
      class="mchip modelstats__btn"
      aria-expanded={open}
      aria-controls="{uid}-table"
      onclick={() => (open ? close(false) : (open = true))}
    >
      {label}
    </button>
    {#if open}
      <div class="modelstats__panel" id="{uid}-table" bind:this={panel}>
        <table>
          <caption>Per model, for these rows</caption>
          <thead>
            <tr>
              <th scope="col">Model</th>
              <th scope="col" class="num">Interactions</th>
              <th scope="col" class="num">Answered</th>
              <th scope="col" class="num">Refused</th>
              <th scope="col" class="num">Errors</th>
              <th scope="col" class="num">p50</th>
              <th scope="col" class="num">Tool calls</th>
            </tr>
          </thead>
          <tbody>
            {#each sorted as m (m.model)}
              <tr>
                <th scope="row" class="mono">{m.model}</th>
                <td class="num mono">{m.count}</td>
                <td class="num mono">{m.answered}</td>
                <td class="num mono">{m.refused}</td>
                <td class="num mono">{m.errors}</td>
                <td class="num mono">{m.p50_ms.toLocaleString('en')} ms</td>
                <td class="num mono">{m.tool_calls}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<style>
  /* Takes what the line leaves: chips that would wrap fall into the clipped
     second row, so the line stays one row and the button stays in reach. */
  .modelstats {
    flex: 1 1 0;
    min-width: 0;
    container-type: inline-size;
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .modelstats__chips {
    flex: 0 1 auto;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0 0.4rem;
    height: 1.75rem;
    margin: 0;
    padding: 0;
    overflow: hidden;
    list-style: none;
  }

  .modelstats__chip {
    display: inline-flex;
    min-width: 12rem;
    align-items: center;
    gap: 0.5rem;
    max-width: 100%;
    height: 1.75rem;
    padding: 0 0.7rem;
    border-radius: 999px;
    background: var(--chip-fill);
    font-size: var(--fs-mini);
    white-space: nowrap;
  }

  .modelstats__name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-weight: 600;
  }

  .modelstats__more {
    position: relative;
    flex: none;
  }

  /* Too narrow for one whole chip beside the button: the button alone, so
     no chip is ever shown cut down to a sliver. */
  @container (max-width: 24rem) {
    .modelstats__chips {
      display: none;
    }
  }

  /* Phones: the button keeps to the line's end, so the table opens leftward on screen. */
  :global(.frame--phone) .modelstats__more {
    margin-left: auto;
  }

  .modelstats__btn {
    border-radius: 999px;
  }

  .modelstats__panel {
    position: absolute;
    top: calc(100% + 0.4rem);
    right: 0;
    z-index: 30;
    max-width: min(44rem, calc(100vw - 2rem));
    overflow-x: auto;
    padding: 0.5rem 0.8rem 0.6rem;
    border: 2px solid var(--win);
    border-radius: var(--r);
    background: var(--surface);
    box-shadow: var(--shadow);
  }

  .modelstats__panel caption {
    padding-bottom: 0.3rem;
    text-align: left;
    font-family: var(--display);
    font-weight: 700;
  }

  .modelstats__panel th,
  .modelstats__panel td {
    white-space: nowrap;
  }
</style>
