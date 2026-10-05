<!--
  Finds a built-in word to allow again (Config → Profanity): a typed search
  over the built-in list with the matches listed under it, so the whole list
  never shows as a wall of words. Arrow keys move, Enter picks, Escape
  clears; focus stays in the field (aria-activedescendant), as in the
  dropdowns' search.
-->
<script lang="ts">
  import '@kanade/ui/styles/select.scss';
  import { matches } from './profanity';

  let {
    words,
    taken,
    onpick,
    describedby,
  }: {
    /** The built-in list. */
    words: string[];
    /** Already allowed: never offered again. */
    taken: string[];
    onpick: (word: string) => void;
    describedby?: string;
  } = $props();
  const uid = $props.id();

  let text = $state('');
  let active = $state(0);
  const found = $derived(matches(words, taken, text));
  const expanded = $derived(found.length > 0);
  const left = $derived(words.filter((w) => !taken.includes(w)).length);

  function pick(word: string) {
    onpick(word);
    text = '';
    active = 0;
  }

  function key(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (!expanded) return;
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      active = (active + step + found.length) % found.length;
    } else if (event.key === 'Enter') {
      // Never submits the section: Enter here only picks.
      if (!text.trim()) return;
      event.preventDefault();
      const word = found[active];
      if (word) pick(word);
    } else if (event.key === 'Escape' && text) {
      event.preventDefault();
      event.stopPropagation();
      text = '';
      active = 0;
    }
  }
</script>

<div class="wordpick">
  <label class="vh" for="{uid}-find">Find a built-in word to allow again</label>
  <input
    id="{uid}-find"
    class="wordpick__find"
    type="search"
    role="combobox"
    aria-autocomplete="list"
    aria-expanded={expanded}
    aria-controls="{uid}-list"
    aria-activedescendant={expanded ? `${uid}-o${active}` : undefined}
    aria-describedby="{uid}-count{describedby ? ` ${describedby}` : ''}"
    placeholder="find a built-in word…"
    autocomplete="off"
    spellcheck="false"
    bind:value={text}
    oninput={() => (active = 0)}
    onkeydown={key}
  />
  <!-- The list is always in the DOM (aria-controls must resolve); it is empty until something is typed. -->
  <div class="dd-list wordpick__list" id="{uid}-list" role="listbox" aria-label="Built-in words" hidden={!expanded}>
    {#each found as word, index (word)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
      <div
        class="dd-opt wordpick__opt mono"
        class:dd-opt--act={index === active}
        role="option"
        id="{uid}-o{index}"
        aria-selected={index === active}
        onmousedown={(event) => event.preventDefault()}
        onclick={() => pick(word)}
      >
        {word}
      </div>
    {/each}
  </div>
  <p class="settings__cardnote" id="{uid}-count" aria-live="polite">
    {#if text.trim() && !expanded}No built-in word left to allow matches “{text.trim()}”.{:else}{left} of {words.length} built-in words still blocked · type to find one{/if}
  </p>
</div>

<style>
  .wordpick {
    display: grid;
    gap: 6px;
    max-width: 22rem;
  }

  .wordpick__find {
    min-height: 40px;
  }

  .wordpick__list {
    padding: 4px;
    border-radius: 16px;
    background: var(--surface);
    box-shadow: inset 0 0 0 1.5px var(--line);
  }

  .wordpick__list[hidden] {
    display: none;
  }

  .wordpick__opt {
    min-height: 36px;
  }
</style>
