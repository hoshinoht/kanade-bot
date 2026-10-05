<!--
  Config → Profanity (user decision 2026-10-05): the chat profanity guardrail.
  The two checks are switches that apply at once (with Undo); the word lists
  and the deflection line save together with the bar, lists replaced whole.
  Refusals stay inline. Applies to the next question; nothing restarts.
-->
<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { Icon, LiveRegion } from '@kanade/ui';
  import { tick } from 'svelte';
  import { changes } from './dirty';
  import { check, draftOf, MAX_LINE_CHARS, wordProblem, wordsOf, type ProfanityField } from './profanity';
  import type { Save } from './save';
  import SaveBar from './SaveBar.svelte';
  import SettingsPanel from './SettingsPanel.svelte';
  import SwitchCard from './SwitchCard.svelte';
  import WordPicker from './WordPicker.svelte';

  let { profanity, save }: { profanity: ConfigView['profanity']; save: Save } = $props();
  const uid = $props.id();

  // Seeded from the saved values once; a refused save keeps the edits to fix.
  // svelte-ignore state_referenced_locally
  let draft = $state(draftOf(profanity));
  /** The add field: typed words count as drafted even before Add, as in Pings. */
  let typed = $state('');
  let error = $state('');
  let bad = $state<ProfanityField | null>(null);
  let saving = $state(false);
  let announce = $state('');
  let addField: HTMLInputElement | undefined = $state();
  let form: HTMLFormElement | undefined = $state();

  const list = (words: string[]) => (words.length ? words.join(', ') : 'none');
  const drafted = $derived([...new Set([...draft.extra_words, ...wordsOf(typed)])]);
  const pending = $derived(
    changes([
      { label: 'Extra words', from: list(profanity.extra_words), to: list(drafted) },
      { label: 'Allowed again', from: list(profanity.allowed_words), to: list(draft.allowed_words) },
      { label: 'Deflection line', from: profanity.deflection_line, to: draft.deflection_line.trim() },
    ]),
  );
  const lineLength = $derived([...draft.deflection_line.trim()].length);

  function discard() {
    draft = draftOf(profanity);
    typed = '';
    error = '';
    bad = null;
  }

  /** Moves the add field's words into chips; false (with the reason shown) when one cannot be added. */
  function add(): boolean {
    const words = wordsOf(typed);
    if (!words.length) return true;
    const problem = words.map((w) => wordProblem(w, profanity)).find(Boolean);
    if (problem) {
      bad = 'extra';
      error = problem;
      return false;
    }
    const fresh = words.filter((w, i) => !draft.extra_words.includes(w) && words.indexOf(w) === i);
    draft.extra_words.push(...fresh);
    typed = '';
    if (bad === 'extra') {
      bad = null;
      error = '';
    }
    const repeated = words.length - fresh.length;
    announce = [fresh.length ? `Added ${fresh.join(', ')}.` : '', repeated ? 'Already listed words were skipped.' : ''].filter(Boolean).join(' ');
    return true;
  }

  function addKeydown(event: KeyboardEvent) {
    if (event.key !== 'Enter' || !typed.trim()) return;
    event.preventDefault();
    add();
  }

  async function removeFrom(which: 'extra_words' | 'allowed_words', index: number) {
    const [gone] = draft[which].splice(index, 1);
    announce = `Removed ${gone}.`;
    await tick();
    // Focus stays in the list: the next chip's ×, else the previous one, else the field that adds.
    const left = form?.querySelectorAll<HTMLButtonElement>(`[data-words="${which}"] .words__remove`) ?? [];
    const fallback = which === 'extra_words' ? addField : form?.querySelector<HTMLInputElement>('[role="combobox"]');
    (left[Math.min(index, left.length - 1)] ?? fallback)?.focus({ preventScroll: true });
  }

  function allow(word: string) {
    if (draft.allowed_words.includes(word)) return;
    draft.allowed_words.push(word);
    announce = `${word} will be allowed again once saved.`;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!add()) return;
    const checked = check(draft, profanity);
    if (!('value' in checked)) {
      error = checked.error;
      bad = checked.field;
      return;
    }
    bad = null;
    saving = true;
    error = await save({ profanity: checked.value }, 'Profanity words saved; the next question uses them.');
    saving = false;
    if (error) {
      // The server's refusal names the list or the line it is about.
      bad = /deflection line/i.test(error) ? 'line' : /allowed|not on the built-in/i.test(error) ? 'allowed' : 'extra';
      return;
    }
    // The saved object is the truth (the line comes back trimmed).
    draft = draftOf(profanity);
  }

  const guard = (on: boolean, side: 'questions' | 'replies') =>
    save({ profanity: side === 'questions' ? { check_questions: on } : { check_replies: on } }, on ? `Checking ${side} again.` : `No longer checking ${side}.`, {
      patch: { profanity: side === 'questions' ? { check_questions: !on } : { check_replies: !on } },
      done: on ? `No longer checking ${side}.` : `Checking ${side} again.`,
    });
</script>

{#snippet chips(which: 'extra_words' | 'allowed_words', label: string)}
  <ul class="words" aria-label={label} data-words={which}>
    {#each draft[which] as word, index (word)}
      <li class="words__chip mono">
        {word}<button class="words__remove" type="button" aria-label="Remove {word}" onclick={() => void removeFrom(which, index)}
          ><span aria-hidden="true">×</span></button
        >
      </li>
    {/each}
  </ul>
{/snippet}

<SettingsPanel title="Profanity">
  {#snippet lead()}Words the chatbot won't take or say. A hit sends the deflection line instead of an answer.{/snippet}
  <SwitchCard
    title={() => 'Check questions'}
    on={profanity.check_questions}
    confirm="Stop checking questions?"
    action={(next) => (next ? 'Check questions' : 'Stop checking questions')}
    apply={(on) => guard(on, 'questions')}
    >A question that uses a listed word gets the deflection line, with no model call.</SwitchCard
  >
  <SwitchCard
    title={() => 'Check replies'}
    on={profanity.check_replies}
    confirm="Stop checking replies?"
    action={(next) => (next ? 'Check replies' : 'Stop checking replies')}
    apply={(on) => guard(on, 'replies')}
    >A reply that uses one is retried once; if the retry still does, the deflection line is sent instead.</SwitchCard
  >

  <form class="profanity" id="{uid}-form" bind:this={form} onsubmit={submit} aria-label="Profanity words" novalidate>
    <fieldset class="settings__card" data-fid="cfg-card" aria-describedby="{uid}-extra-note">
      <legend class="settings__cardtitle">Extra blocked words</legend>
      <p class="settings__cardnote" id="{uid}-extra-note">Blocked on top of the built-in list. Single words of letters; a trailing “s” matches too.</p>
      {#if draft.extra_words.length}
        {@render chips('extra_words', 'Extra blocked words')}
      {:else}
        <p class="note">None yet: only the built-in list is blocked.</p>
      {/if}
      <div class="words__add">
        <label class="vh" for="{uid}-add">Add blocked words</label>
        <input
          id="{uid}-add"
          class="mono words__field"
          class:settings__changed={typed.trim() !== ''}
          bind:this={addField}
          bind:value={typed}
          onkeydown={addKeydown}
          placeholder="add a word…"
          autocomplete="off"
          spellcheck="false"
          aria-invalid={bad === 'extra'}
          aria-describedby={bad === 'extra' ? `${uid}-error` : undefined}
        /><button class="btn words__addbtn" type="button" onclick={() => add()}>Add</button>
      </div>
    </fieldset>

    <fieldset class="settings__card" data-fid="cfg-card" aria-describedby="{uid}-allowed-note">
      <legend class="settings__cardtitle">Built-in words allowed again</legend>
      <p class="settings__cardnote" id="{uid}-allowed-note">Take a built-in word off the list for this server, for example a word your guild uses harmlessly.</p>
      {#if draft.allowed_words.length}
        {@render chips('allowed_words', 'Built-in words allowed again')}
      {:else}
        <p class="note">None: every built-in word is blocked.</p>
      {/if}
      <WordPicker words={profanity.builtin_words} taken={draft.allowed_words} onpick={allow} describedby={bad === 'allowed' ? `${uid}-error` : undefined} />
    </fieldset>

    <fieldset class="settings__card" data-fid="cfg-card">
      <legend class="settings__cardtitle">Deflection line</legend>
      <label class="field">
        <span>Sent instead</span>
        <textarea
          class="words__line"
          class:settings__changed={draft.deflection_line.trim() !== profanity.deflection_line}
          rows="2"
          maxlength={MAX_LINE_CHARS}
          bind:value={draft.deflection_line}
          aria-invalid={bad === 'line'}
          aria-describedby="{uid}-line-note{bad === 'line' ? ` ${uid}-error` : ''}"
        ></textarea>
      </label>
      <p class="settings__cardnote" id="{uid}-line-note">
        One line, {MAX_LINE_CHARS} characters at most (<span class="mono">{lineLength}</span> now). Also the safe line for a reply that still uses a listed
        word after its retry. It can't use a listed word itself.
      </p>
    </fieldset>
  </form>
  {#if error}<p class="field__error" id="{uid}-error" role="alert">{error}</p>{/if}
  <LiveRegion message={announce} />
  <p class="settings__box">
    <Icon name="info" /><span
      >Reminder headings and nudge lead-ins are checked against the same list. Deflected exchanges stay out of later chat context, so the bot never
      builds on them. Changes apply to the next question.</span
    >
  </p>
  {#snippet bar()}
    <SaveBar form="{uid}-form" label="Save profanity" changes={pending} {saving} ondiscard={discard} />
  {/snippet}
</SettingsPanel>

<style>
  .profanity {
    display: grid;
    gap: 0.875rem;
  }

  .profanity legend {
    float: left;
    width: 100%;
    padding: 0;
  }

  .profanity legend + * {
    clear: both;
  }

  /* The card's gap spaces its lines; the shared note margin would double it. */
  .profanity .note {
    margin: 0;
  }

  /* Word chips (the Pings countdown chips): mono, tonal, a round × at the end. */
  .words {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .words__chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 100%;
    min-height: 28px;
    margin: 0;
    padding: 0 2px 0 10px;
    border-radius: 14px;
    background: var(--chip-fill);
    color: var(--ink);
    font-size: var(--fs-small);
    font-weight: 500;
    overflow-wrap: anywhere;
  }

  .words__remove {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--dim-text);
    font: inherit;
    font-size: var(--fs-body);
    line-height: 1;
    cursor: pointer;
  }

  .words__remove:hover {
    background: var(--row-hover);
    color: var(--ink);
  }

  .words__add {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .words__field {
    width: min(14rem, 100%);
    min-height: 40px;
  }

  .words__addbtn {
    min-height: 40px;
    border-radius: 20px;
  }

  .words__line {
    resize: vertical;
  }
</style>
