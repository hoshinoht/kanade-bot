<!--
  Guides (module `guides`; boards G_List, G_PhoneList, G_EditorStates): the
  Drafts window, titled Guides, with New guide in the title bar, the guides
  as a listbox (portrait, title, channel, messages, why it needs a look, the
  status chip and a time) beside the open guide (`GuideView`). Deep link
  `?item=`. Phones show the list, then the guide with "‹ Guides" in the top
  bar. Nothing posts until an admin publishes.
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/core/panes.scss';
  import '@kanade/ui/styles/screens/inbox.scss';
  import '@kanade/ui/styles/screens/history.scss';
  import '@kanade/ui/styles/screens/drafts.scss';
  import '@kanade/ui/styles/screens/reminders.scss';
  import '@kanade/ui/styles/screens/guides.scss';
  import type { Channel, GuideList, GuideSaved } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState, Modal, PendingLabel, SINGLE_PANE_QUERY, Select, StateNote, StatusChip, Toaster, enter, watchMedia } from '@kanade/ui';
  import { tick } from 'svelte';
  import { Resource, send } from '../resource.svelte';
  import OptionList from '../shared/OptionList.svelte';
  import { getChrome } from '../shell/chrome';
  import type { AdminWeek } from '../store.svelte';
  import { GUIDES_PATH, STATUS, reasonText, rowWhen, statusWord } from './guides';
  import GuideView from './GuideView.svelte';
  import { channelOptions } from './channels';

  let {
    store,
    toaster,
    item = '',
    onselect,
  }: {
    store: AdminWeek;
    toaster: Toaster;
    item?: string;
    /** Pushes a history entry when `open` (phones: Back returns to the list). */
    onselect?: (item: string, open: boolean) => void;
  } = $props();

  const list = new Resource<GuideList>(GUIDES_PATH, { topics: ['settings'], keys: (l) => l.guides.map((g) => g.id) });
  const channels = new Resource<Channel[]>('/api/admin/channels');
  $effect(() => list.watch());
  $effect(() => void channels.load());
  // A run in progress moves its chip ("Publishing 3 of 7"): re-read while any guide publishes.
  const publishing = $derived(list.data?.guides.some((g) => g.status === 'publishing') ?? false);
  $effect(() => {
    if (!publishing) return;
    const timer = setInterval(() => void list.refresh(), 1_500);
    return () => clearInterval(timer);
  });

  const uid = $props.id();
  const items = $derived(list.data?.guides ?? []);
  const timeZone = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');
  const now = $derived(store.week?.generated_at ?? '');

  let phone = $state(false);
  $effect(() => watchMedia(SINGLE_PANE_QUERY, (matches) => (phone = matches)));
  const chosen = $derived(items.find((g) => g.id === item) ?? (phone ? undefined : items[0]));
  const chosenId = $derived(chosen?.id ?? null);

  // The phone frame's open guide: "‹ Guides" in the top bar, no window chrome.
  const chrome = getChrome();
  const compact = $derived(phone && Boolean(chosenId) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Guides', name: 'Back to the list (Guides)', go: () => void leave(() => leaveDetail()) });
    return () => chrome.back(null);
  });

  const pushedHere = () => (history.state as { guideDetail?: boolean } | null)?.guideDetail === true;
  function leaveDetail() {
    if (phone && pushedHere()) history.back();
    else onselect?.('', false);
  }

  // Leaving a guide with unsaved edits asks first (G_EditorStates "Leaving with unsaved changes").
  let view = $state<{ unsaved: () => string[] }>();
  let leaving = $state<{ go: () => void; changes: string[] } | null>(null);
  function leave(go: () => void) {
    const changes = view?.unsaved() ?? [];
    if (!changes.length) return go();
    leaving = { go, changes };
  }
  $effect(() => {
    const guard = (event: BeforeUnloadEvent) => {
      if (view?.unsaved().length) event.preventDefault();
    };
    window.addEventListener('beforeunload', guard);
    return () => window.removeEventListener('beforeunload', guard);
  });

  let listRef = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detailEl = $state<HTMLDivElement>();
  let shown = '';
  $effect(() => {
    const key = chosenId ?? '';
    const was = shown;
    shown = key;
    if (!phone || key === was) return;
    if (chosenId && !was) void tick().then(() => detailEl?.focus({ preventScroll: true }));
    else if (!chosenId && was) void listRef?.focusOn(was);
  });

  function pick(id: string, open: boolean) {
    if (id === chosenId) return;
    leave(() => onselect?.(id, open && phone));
  }

  // New guide: a title and, optionally, the channel (G_EditorStates "New guide").
  let newOpen = $state(false);
  let newTitle = $state('');
  let newChannel = $state('');
  let newBusy = $state(false);
  let newError = $state('');
  const newOptions = $derived(channelOptions(channels.data ?? [], items, null));
  async function create(event?: SubmitEvent) {
    event?.preventDefault();
    newBusy = true;
    const made = await send((c) => c.post<GuideSaved>(GUIDES_PATH, { title: newTitle.trim(), channel_id: newChannel || null }));
    newBusy = false;
    if (!made.ok) {
      newError = made.message;
      return;
    }
    newOpen = false;
    toaster.show({ message: `${newTitle.trim()} started; nothing posts until you publish.`, tone: 'ok' });
    await list.load();
    onselect?.(made.value.id, phone);
  }
  function askNew() {
    leave(() => {
      newTitle = '';
      newChannel = '';
      newError = '';
      newOpen = true;
    });
  }

  async function deleted(message: string) {
    toaster.show({ message, tone: 'ok' });
    await list.load();
    leaveDetail();
  }
</script>

<PageLine title={list.data ? 'Guides' : ''} class={chrome?.phone ? 'pageline--echo' : ''}>
  <h1>{#if list.data}<span class="pageline__num">{items.length}</span> guide{items.length === 1 ? '' : 's'}{:else}Guides{/if}</h1>
  <p class="pageline__context">Discord guide posts; nothing posts until you publish</p>
</PageLine>

{#snippet newButton(where: 'bar' | 'note')}
  <button class={where === 'bar' ? 'btn drafts__newbtn' : 'btn btn--primary drafts__notebtn'} type="button" onclick={askNew}><Icon name="plus" />New guide</button>
{/snippet}

<section data-fid="window" class="card tabs drafts guides window-fill" class:drafts--compact={compact} aria-label="Guides">
  <div class="card__head tabs__strip" data-fid="window-bar">
    {#if chrome?.phone}<h2 class="vh">Guides</h2>{:else}<h2 class="card__title" data-fid="window-title">Guides</h2>{/if}
    {#if list.data}<div class="drafts__new" data-fid="guides-new">{@render newButton('bar')}</div>{/if}
  </div>
  <div class="drafts__body" class:drafts__body--empty={list.data && !items.length} class:drafts__body--waiting={!list.data}>
    {#if list.error && !list.data}
      <LoadError thing="guides" reason="{list.error} Your guides are kept on the server; nothing was lost." onretry={() => void list.load()} />
    {:else if !list.data}
      {#if !phone}<div class="drafts__list drafts__list--waiting" aria-busy="true"></div>{/if}
      <div class="state-pane drafts__loading" data-fid="state-pane"><div class="drafts__loader"><LoadingState text="Loading guides…" /></div></div>
    {:else if !items.length}
      <div class="inbox-empty" data-fid="empty-body">
        {#snippet ways()}{@render newButton('note')}{/snippet}
        <StateNote icon="book-open" title="No guides yet" actions={ways}>
          Write a guide as text and boss blocks, preview it as Discord shows it, then publish it to a channel. Nothing posts until you publish.
        </StateNote>
      </div>
    {:else}
      <div class="drafts__list" hidden={phone && Boolean(chosenId)}>
        <OptionList
          bind:this={listRef}
          label="Guides"
          {items}
          selected={chosenId ?? ''}
          follow={!phone}
          onpick={pick}
          fresh={list.fresh}
          fid="guides-row"
          listFid="guides-list"
          rowClass={() => 'inbox__option inbox__option--art drafts__row'}
          activeClass="inbox__option--active"
        >
          {#snippet option(g)}
            {@const why = g.status === 'publishing' ? null : reasonText(g)}
            {@const tone = STATUS[g.status].tone}
            <span class="inbox__art drafts__art" aria-hidden="true">{#if g.portrait}<img class="portrait portrait--md" src={g.portrait} alt="" />{/if}</span>
            <span class="inbox__lines">
              <span class="inbox__what">{g.title}</span>
              <span class="inbox__meta">
                {#if g.channel_id}<span class="mono">{g.channel_name ?? 'a channel Kanade can’t see'}</span> ·
                  {#if g.messages}<span class="guide-row__n"><b class="mono drafts__count">{g.messages}</b> message{g.messages === 1 ? '' : 's'}</span>{:else}not posted yet{/if}
                {:else}No channel yet{/if}
              </span>
              {#if why}
                <span class="inbox__meta guide-row__why guide-row__why--{tone}"><Icon name={tone === 'risk' ? 'alert-triangle' : tone === 'warn' ? 'refresh-cw' : 'clock'} /><span>{why}</span></span>
              {/if}
              <span class="inbox__badges drafts__state">
                <StatusChip {tone}>{statusWord(g)}</StatusChip>
                <span class="mono drafts__edited">{rowWhen(g, timeZone, now)}</span>
              </span>
            </span>
          {/snippet}
        </OptionList>
      </div>
      <div class="drafts__detail" hidden={!chosenId} tabindex="-1" bind:this={detailEl} {@attach enter(chosenId)}>
        {#if chosen}
          {#key chosen.id}
            <GuideView
              bind:this={view}
              id={chosen.id}
              row={chosen}
              rows={items}
              channels={channels.data ?? []}
              {timeZone}
              {now}
              bar={phone}
              {toaster}
              onchanged={() => void list.refresh()}
              ondeleted={(message) => void deleted(message)}
            />
          {/key}
        {/if}
      </div>
    {/if}
  </div>
</section>

<Modal bind:open={newOpen} title="New guide" eyebrow="Guides" narrow>
  <form id="{uid}-new" onsubmit={create}>
    <label class="field">
      <span>Title</span>
      <input bind:value={newTitle} maxlength="100" required placeholder="e.g. Bellona basics" aria-describedby="{uid}-new-err" />
    </label>
    <div class="field">
      <span id="{uid}-new-ch">Channel</span>
      <Select bind:value={newChannel} options={newOptions} label="Channel" placeholder="Choose a channel" noun="channels" />
      <small class="note">Optional now; needed to publish. One guide per channel.</small>
    </div>
    <p class="field__error" id="{uid}-new-err" role="alert">{newError}</p>
  </form>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button class="btn btn--primary" type="submit" form="{uid}-new" disabled={newBusy || !newTitle.trim()}><PendingLabel pending={newBusy} label="Starting…"><Icon name="plus" />Start guide</PendingLabel></button>
  {/snippet}
</Modal>

<Modal bind:open={() => leaving !== null, (value) => !value && (leaving = null)} title="Leave without saving?" eyebrow="Guides" narrow alert describedby="{uid}-leave">
  <p id="{uid}-leave">Your {leaving?.changes.length === 1 ? 'change' : `${leaving?.changes.length ?? 0} changes`} to {chosen?.title ?? 'this guide'} {leaving?.changes.length === 1 ? 'isn’t' : 'aren’t'} saved: {leaving?.changes.join(', ')}. Nothing was posted.</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Stay</button>
    <button
      class="btn btn--danger"
      type="button"
      onclick={() => {
        const go = leaving?.go;
        leaving = null;
        go?.();
      }}>Discard changes</button
    >
  {/snippet}
</Modal>
