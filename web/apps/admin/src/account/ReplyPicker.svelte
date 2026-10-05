<!--
  The reply-style picker (board AdminAccount-Reply): a searchable, scrolling
  radio list of the public reply profiles with each one's voice line (a
  full-screen sheet on phones). It changes only the admin's own saved style;
  when a role sets the style it says so first, since the role keeps winning.
-->
<script lang="ts">
  import type { Persona, ReplyStyle } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState, Modal } from '@kanade/ui';
  import { tick } from 'svelte';
  import { Resource } from '../resource.svelte';
  import { DEFAULT_STYLE, inEffectName, savedKey, styleChoices } from './account';

  let {
    open = $bindable(false),
    style,
    personas,
    saving = false,
    error = '',
    onsave,
  }: {
    open: boolean;
    style: ReplyStyle;
    personas: Resource<Persona[]>;
    saving?: boolean;
    error?: string;
    onsave: (key: string, name: string) => void;
  } = $props();

  const uid = $props.id();
  let query = $state('');
  const saved = $derived(savedKey(style));
  let picked = $state('');
  let search = $state<HTMLInputElement>();

  // Each opening starts from the saved style with an empty search, the search focused.
  let wasOpen = false;
  $effect(() => {
    if (open && !wasOpen) {
      picked = saved;
      query = '';
      void tick().then(() => search?.focus());
    }
    wasOpen = open;
  });

  const all = $derived(personas.data ?? []);
  const shown = $derived(styleChoices(all, query));
  const choice = $derived(styleChoices(all, '').find((p) => p.key === picked) ?? null);
  const unchanged = $derived(picked === saved);

  function onSearchKey(event: KeyboardEvent) {
    // Down from the search moves into the list, as a combobox would.
    if (event.key !== 'ArrowDown') return;
    const first = document.querySelector<HTMLInputElement>(`#${uid}-list input:checked`) ?? document.querySelector<HTMLInputElement>(`#${uid}-list input`);
    if (!first) return;
    event.preventDefault();
    first.focus();
  }
</script>

<Modal bind:open title="Reply style" className="reply-picker" flush>
  <div class="reply-picker__top">
    <p>Changes how Kanade talks to you in chat. Schedule facts stay exactly the same.</p>
    <div class="reply-picker__search" role="search">
      <Icon name="search" />
      <label class="vh" for="{uid}-q">Find a style</label>
      <input
        id="{uid}-q"
        type="search"
        bind:this={search}
        bind:value={query}
        placeholder="Find a style by name or voice"
        autocomplete="off"
        spellcheck="false"
        aria-controls="{uid}-list"
        onkeydown={onSearchKey}
      />
    </div>
    {#if style.source === 'role'}
      <p class="settings__box reply-picker__warn" role="note">
        <Icon name="lock" />
        <span
          >Your {#if style.role_name}<b>{style.role_name}</b>{/if} role sets <b>{inEffectName(style)}</b>, and role settings come first. Your pick is saved and
          takes over once no role sets a style.</span
        >
      </p>
    {/if}
    <div class="reply-picker__count">
      <span class="cap" id="{uid}-all">All styles</span><span class="account-session__mono">{personas.data ? shown.length : ''}</span>
      <span class="reply-picker__order">A to Z</span>
    </div>
  </div>
  <fieldset class="reply-picker__list" id="{uid}-list" aria-labelledby="{uid}-all">
    {#if personas.error && !personas.data}
      <LoadError thing="the reply styles" reason={personas.error} onretry={() => void personas.load()} />
    {:else if !personas.data}
      <LoadingState text="Loading reply styles…" />
    {:else if shown.length}
      <div class="account-grp">
        {#each shown as option (option.key)}
          <label class="account-row reply-picker__option">
            <input type="radio" name="{uid}-style" value={option.key} bind:group={picked} />
            <span class="account-row__text">
              <span class="account-row__title">{option.name}{#if option.key === saved}<span class="reply-picker__saved account-row__sub">· saved</span>{/if}</span>
              {#if option.voice}<span class="account-row__sub reply-picker__voice">{option.voice}</span>{/if}
            </span>
          </label>
        {/each}
      </div>
    {:else}
      <p class="reply-picker__empty">No style matches “{query}”.</p>
    {/if}
  </fieldset>
  {#snippet footer(close)}
    <span class="reply-picker__now">Saved now: <b>{style.saved?.name ?? DEFAULT_STYLE.name}</b></span>
    {#if error}<p class="field__error reply-picker__error" role="alert">{error}</p>{/if}
    <button type="button" class="btn" onclick={close}>Cancel</button>
    <button
      type="button"
      class="btn btn--primary btn--key"
      aria-disabled={unchanged || saving || !choice}
      onclick={() => {
        if (!unchanged && !saving && choice) onsave(choice.key, choice.name);
      }}>{saving ? 'Saving…' : unchanged || !choice ? 'Saved' : `Save ${choice.name}`}</button
    >
  {/snippet}
</Modal>
