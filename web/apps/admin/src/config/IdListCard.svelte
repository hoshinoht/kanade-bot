<!--
  One Channels list (watched channels, watched categories or chat
  categories): its own form and Save, which sends only this list (an
  explicit list save). Channels are picked from the guild directory;
  categories are typed as ids. A refusal stays inline under the card.
-->
<script lang="ts">
  import type { IdListSource } from '@kanade/api-types';
  import { LiveRegion, MultiSelect, PendingLabel } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { tick, untrack } from 'svelte';
  import { dirtyReport } from './dirty';

  let {
    title,
    env,
    ids,
    source,
    noun,
    options,
    save,
    children,
  }: {
    title: string;
    /** The env variable that seeds the list. */
    env: string;
    ids: string[];
    source: IdListSource;
    /** "channels" or "categories", for counts and labels. */
    noun: string;
    /** The directory's choices; absent, ids are typed. */
    options?: { value: string; label: string }[];
    /** Saves the whole list; resolves to '' or the refusal. */
    save: (ids: string[]) => Promise<string>;
    children: import('svelte').Snippet;
  } = $props();
  const uid = $props.id();

  // A refused save keeps the draft; an untouched draft follows the saved list.
  // svelte-ignore state_referenced_locally
  let draft = $state<string[]>([...ids]);
  // svelte-ignore state_referenced_locally
  let base = [...ids];
  let typed = $state('');
  let error = $state('');
  let saving = $state(false);
  let announce = $state('');
  let addField: HTMLInputElement | undefined = $state();
  let chipList: HTMLUListElement | undefined = $state();

  const same = (a: string[], b: string[]) => a.length === b.length && a.every((id, i) => id === b[i]);
  const dirty = $derived(!same(draft, ids) || typed.trim() !== '');
  // Saved ids the directory no longer lists stay pickable, shown by id.
  const choices = $derived(options ? [...options, ...draft.filter((id) => !options.some((o) => o.value === id)).map((id) => ({ value: id, label: id }))] : []);

  // No remount on a new list (that dropped focus after Save): resync in place.
  $effect(() => {
    const next = [...ids];
    untrack(() => {
      if (same(next, base)) return;
      if (same(draft, base) && !typed.trim()) draft = [...next];
      base = next;
    });
  });

  const report = dirtyReport();
  $effect(() => {
    report?.set(`channels/${uid}`, dirty);
    return () => report?.set(`channels/${uid}`, false);
  });

  const parts = (text: string) => text.split(/[\s,]+/).filter(Boolean);
  const RULE = 'Ids are whole numbers, separated by commas or spaces.';

  /** Moves typed ids into chips; false (and the rule shown) when one is not a number. */
  function add(): boolean {
    if (!typed.trim()) return true;
    if (!parts(typed).every((id) => /^\d+$/.test(id))) {
      error = RULE;
      return false;
    }
    const fresh = parts(typed).filter((id, i, all) => !draft.includes(id) && all.indexOf(id) === i);
    draft = [...draft, ...fresh];
    typed = '';
    if (error === RULE) error = '';
    announce = fresh.length ? `Added ${fresh.join(', ')}.` : 'Already listed.';
    return true;
  }

  async function remove(index: number) {
    const [gone] = draft.splice(index, 1);
    announce = `Removed ${gone}.`;
    await tick();
    const left = chipList?.querySelectorAll<HTMLButtonElement>('.idlist__remove') ?? [];
    (left[Math.min(index, left.length - 1)] ?? addField)?.focus({ preventScroll: true });
  }

  function discard() {
    draft = [...ids];
    typed = '';
    error = '';
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!dirty || saving || !add()) return;
    saving = true;
    error = await save([...draft]);
    saving = false;
    if (!error) {
      draft = [...ids];
      base = [...ids];
    }
  }
</script>

<form class="settings__card idlist" data-fid="cfg-card" aria-labelledby="{uid}-t" onsubmit={submit}>
  <div class="settings__cardhead">
    <h4 class="settings__cardtitle" id="{uid}-t">{title}</h4>
    <span class="capchip idlist__source" data-source={source}
      >{source === 'saved' ? 'Saved here' : 'From'} <code class="mono">{env}</code>{source === 'saved' ? ' is ignored' : ''}</span
    >
  </div>
  <p class="settings__cardnote">{@render children()}</p>
  {#if options}
    <MultiSelect
      size="field"
      label={title}
      fullLabel="{title} to keep"
      values={draft}
      options={choices}
      {noun}
      onchange={(values) => (draft = choices.map((c) => c.value).filter((id) => values.includes(id)))}
    />
  {:else}
    <div class="idlist__field" role="group" aria-labelledby="{uid}-t">
      <ul class="idlist__chips" aria-label={title} bind:this={chipList}>
        {#each draft as id, index (id)}
          <li class="idlist__chip mono">
            {id}<button class="idlist__remove" type="button" aria-label="Remove {id}" onclick={() => void remove(index)}><span aria-hidden="true">×</span></button>
          </li>
        {:else}
          <li class="idlist__none">None</li>
        {/each}
      </ul>
      <label class="idlist__add"
        ><span class="vh">Add {noun} by id</span><input
          class="mono idlist__addfield"
          bind:this={addField}
          bind:value={typed}
          onkeydown={(event) => {
            if (event.key === 'Enter' && typed.trim()) {
              event.preventDefault();
              add();
            }
          }}
          inputmode="numeric"
          autocomplete="off"
          placeholder="category id"
          aria-invalid={error === RULE}
        /></label
      ><button class="btn idlist__addbtn" type="button" onclick={() => add()}>Add</button>
    </div>
  {/if}
  {#if error}<p class="field__error" role="alert">{error}</p>{/if}
  <LiveRegion message={announce} />
  <div class="idlist__actions">
    {#if dirty}<button class="btn" type="button" disabled={saving} onclick={discard}>Discard</button>{/if}
    <button class="btn btn--primary settings__key" type="submit" aria-disabled={!dirty || saving}
      ><PendingLabel pending={saving} label="Saving…">Save {title.toLowerCase()}</PendingLabel></button
    >
  </div>
</form>

<style>
  .idlist__source code {
    font-size: inherit;
  }

  .idlist__field {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .idlist__chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .idlist__chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 100%;
    height: 26px;
    padding: 0 2px 0 10px;
    border-radius: 13px;
    background: var(--chip-fill);
    color: var(--ink);
    font-size: var(--fs-small);
    overflow-wrap: anywhere;
  }

  .idlist__none {
    color: var(--dim-text);
    font-size: var(--fs-small);
  }

  .idlist__remove {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--dim-text);
    font: inherit;
    line-height: 1;
    cursor: pointer;
  }

  .idlist__remove:hover {
    background: var(--row-hover);
    color: var(--ink);
  }

  .idlist__addfield {
    width: 12rem;
    max-width: 100%;
    height: 32px;
    min-height: 32px;
  }

  .idlist__addbtn {
    min-height: 32px;
    border-radius: 16px;
  }

  .idlist__actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
