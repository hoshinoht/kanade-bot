<script lang="ts" module>
  export interface Command {
    id: string;
    label: string;
    group: string;
    keywords?: string;
    run: () => void;
  }

  /** Every query word must appear; label-prefix matches rank first. */
  export function filterCommands(commands: Command[], query: string): Command[] {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (words.length === 0) return commands;
    return commands
      .map((command) => {
        const hay = `${command.label} ${command.group} ${command.keywords ?? ''}`.toLowerCase();
        if (!words.every((w) => hay.includes(w))) return null;
        const label = command.label.toLowerCase();
        return { command, score: label.startsWith(words[0]!) ? 0 : label.includes(words[0]!) ? 1 : 2 };
      })
      .filter((x): x is { command: Command; score: number } => x !== null)
      .sort((a, b) => a.score - b.score)
      .map((x) => x.command);
  }
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import Icon from './Icon.svelte';

  let { open = $bindable(false), commands }: { open: boolean; commands: Command[] } = $props();

  const uid = $props.id();
  let dialog: HTMLDialogElement;
  let input: HTMLInputElement;
  let query = $state('');
  let active = $state(0);
  let returnTo: HTMLElement | null = null;
  let pending: Command | null = null;

  const results = $derived(filterCommands(commands, query));
  const activeId = $derived.by(() => {
    const current = results[active];
    return current ? `${uid}-opt-${current.id}` : undefined;
  });

  $effect(() => {
    if (open && !dialog.open) {
      returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      query = '';
      active = 0;
      dialog.showModal();
      input.focus();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  $effect(() => {
    if (active >= results.length) active = Math.max(0, results.length - 1);
  });

  async function scrollActive() {
    await tick();
    if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: 'nearest' });
  }

  function choose(command: Command | undefined) {
    if (!command) return;
    pending = command;
    open = false;
  }

  async function handleClose() {
    open = false;
    const command = pending;
    pending = null;
    if (returnTo?.isConnected) returnTo.focus();
    returnTo = null;
    // Run after focus is restored so a command that opens a dialog can take focus itself.
    await tick();
    command?.run();
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (results.length === 0) return;
      const step = event.key === 'ArrowDown' ? 1 : -1;
      active = (active + step + results.length) % results.length;
      void scrollActive();
    } else if (event.key === 'Enter') {
      event.preventDefault();
      choose(results[active]);
    }
  }
</script>

<dialog bind:this={dialog} class="modal palette" aria-labelledby="{uid}-title" onclose={handleClose}>
  <div class="modal__panel">
    <h2 class="vh" id="{uid}-title">Command palette</h2>
    <div class="palette__search">
      <Icon name="search" />
      <input
        bind:this={input}
        bind:value={query}
        type="text"
        role="combobox"
        aria-label="Search commands"
        aria-expanded="true"
        aria-controls="{uid}-list"
        aria-autocomplete="list"
        aria-activedescendant={activeId}
        autocomplete="off"
        spellcheck="false"
        placeholder="Type a command, a run or a colourway"
        oninput={() => (active = 0)}
        onkeydown={onKeydown}
      />
    </div>
    <ul class="palette__list" id="{uid}-list" role="listbox" aria-label="Commands">
      {#each results as command, index (command.id)}
        <!-- Keyboard reaches options through the combobox (aria-activedescendant), not per option. -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          id="{uid}-opt-{command.id}"
          class="palette__option"
          role="option"
          aria-selected={index === active}
          onclick={() => choose(command)}
          onpointermove={() => (active = index)}
        >
          <span>{command.label}</span>
          <span class="palette__group">{command.group}</span>
        </li>
      {/each}
    </ul>
    {#if results.length === 0}
      <p class="palette__empty" role="status">Nothing matches “{query}”.</p>
    {/if}
    <p class="palette__hint">
      <span><kbd class="kbd">↑</kbd> <kbd class="kbd">↓</kbd> choose</span>
      <span><kbd class="kbd">Enter</kbd> run</span>
      <span><kbd class="kbd">Esc</kbd> close</span>
    </p>
  </div>
</dialog>

<style>
  /* Lives with the component so apps that never import it ship none of it. */
  dialog.palette {
    margin-top: min(14dvh, 7rem);
    width: min(36rem, calc(100% - 1.6rem));
  }

  .palette__search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.6rem 0.85rem;
    border-bottom: 2px solid var(--line);
    color: var(--dim);
  }

  .palette__search input {
    flex: 1 1 auto;
    border: 0;
    padding: 0.3rem 0;
    background: transparent;
    font-size: var(--fs-body);
  }

  .palette__search input:focus {
    box-shadow: none;
  }

  .palette__list {
    list-style: none;
    margin: 0;
    padding: 0.35rem;
    max-height: min(50dvh, 22rem);
    overflow-y: auto;
  }

  .palette__option {
    display: flex;
    align-items: baseline;
    gap: 0.6rem;
    padding: 0.45rem 0.6rem;
    border-radius: var(--r-sm);
    cursor: pointer;
  }

  /* Chrome colours, not accent: accent/accent-ink is under 4.5:1 in two colourways. */
  .palette__option[aria-selected="true"] {
    background: var(--win);
    color: var(--win-ink);
  }

  .palette__option[aria-selected="true"] .palette__group {
    color: inherit;
  }

  .palette__group {
    margin-left: auto;
    font-family: var(--mono);
    font-size: var(--fs-micro);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--faint);
  }

  .palette__empty {
    padding: 0.8rem;
    color: var(--dim);
  }

  .palette__hint {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem 0.9rem;
    padding: 0.45rem 0.85rem;
    background: var(--raise);
    border-top: 2px solid var(--line);
    font-size: var(--fs-mini);
    color: var(--dim);
  }
</style>
