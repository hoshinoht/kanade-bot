<!--
  Drafts (boards B_Drafts*, B_PhoneDraft*): the signed-in admin's staged
  schedule changes, one window with Open / Closed title-bar tabs, the list
  beside the open draft and its preview or conflicts pane (the Inbox
  pattern). Nothing in a draft is live until Merge. Deep links: ?tab=&item=.
  Phones show the list, then the draft with "‹ Drafts" in the top bar and
  the decision as a bottom bar.
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/core/panes.scss';
  import '@kanade/ui/styles/screens/inbox.scss';
  import '@kanade/ui/styles/screens/history.scss';
  import '@kanade/ui/styles/screens/drafts.scss';
  import type { Boss, DraftDetail, DraftList as DraftListData, DraftWritten } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState, Modal, PendingLabel, Portrait, SINGLE_PANE_QUERY, StateNote, Toaster, enter, watchMedia } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import { shortAt } from '../history/describe';
  import { Resource, send } from '../resource.svelte';
  import { getChrome } from '../shell/chrome';
  import type { AdminWeek } from '../store.svelte';
  import { closedLabel, DRAFTS_PATH, draftPath, firstRun } from './drafts';
  import DraftList from './DraftList.svelte';
  import DraftView from './DraftView.svelte';

  type Tab = 'open' | 'closed';

  let {
    store,
    toaster,
    tab = '',
    item = '',
    onselect,
    onadd,
  }: {
    store: AdminWeek;
    toaster: Toaster;
    tab?: string;
    item?: string;
    /** Pushes a history entry when `open` (phones: Back returns to the list). */
    onselect?: (tab: Tab, item: string, open: boolean) => void;
    /** "Add a change": stage into the draft and pick a run on the Week board. */
    onadd?: (draft: { id: string; title: string; version: number }) => void;
  } = $props();

  // Draft writes hint `inbox`, a merge also `schedule`; a moved head (any schedule change) moves "Up to date".
  const openList = new Resource<DraftListData>(`${DRAFTS_PATH}?state=open`, { topics: ['inbox', 'schedule'], keys: (l) => l.drafts.map((d) => d.id) });
  const closedList = new Resource<DraftListData>(`${DRAFTS_PATH}?state=closed`, { topics: ['inbox', 'schedule'] });
  $effect(() => openList.watch());
  $effect(() => closedList.watch());

  const uid = $props.id();
  const current = $derived<Tab>(tab === 'closed' ? 'closed' : 'open');
  const list = $derived(current === 'open' ? openList : closedList);
  const items = $derived(list.data?.drafts ?? []);
  const openCount = $derived(openList.data?.drafts.length ?? null);
  const timeZone = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');

  let phone = $state(false);
  $effect(() => watchMedia(SINGLE_PANE_QUERY, (matches) => (phone = matches)));
  const chosen = $derived(items.find((d) => d.id === item) ?? (phone ? undefined : items[0]));
  const chosenId = $derived(chosen?.id ?? null);

  // The first staged run's boss leads each row: read once per draft (rows carry no ops).
  let portraits = $state<Record<string, Boss | null>>({});
  $effect(() => {
    const ids = [...(openList.data?.drafts ?? []), ...(closedList.data?.drafts ?? [])].map((d) => d.id);
    const runs = store.week?.runs ?? [];
    untrack(() => {
      for (const id of ids.filter((id) => !(id in portraits))) {
        portraits[id] = null;
        void send((c) => c.get<DraftDetail>(draftPath(id))).then((read) => {
          const run = read.ok ? firstRun(read.value.ops) : null;
          portraits[id] = runs.find((r) => r.id === run)?.bosses[0] ?? null;
        });
      }
    });
  });

  // The phone frame's open draft: "‹ Drafts" in the top bar, no window chrome.
  const chrome = getChrome();
  const compact = $derived(phone && Boolean(chosenId) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Drafts', name: 'Back to the list (Drafts)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  const pushedHere = () => (history.state as { draftDetail?: boolean } | null)?.draftDetail === true;
  function leaveDetail() {
    if (phone && pushedHere()) history.back();
    else onselect?.(current, '', false);
  }

  const tabEls: Record<string, HTMLButtonElement> = {};
  const TABS: { id: Tab; label: string }[] = [
    { id: 'open', label: 'Open' },
    { id: 'closed', label: 'Closed' },
  ];
  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = TABS[(target + TABS.length) % TABS.length]!;
    onselect?.(next.id, '', false);
    tabEls[next.id]?.focus();
  }

  let listRef = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detailEl = $state<HTMLDivElement>();
  // Phones swap list and detail: focus goes into the draft, and back to its row.
  let shown = '';
  $effect(() => {
    const key = `${current}/${chosenId ?? ''}`;
    const was = shown;
    shown = key;
    if (!phone || key === was || !was.startsWith(`${current}/`)) return;
    const wasId = was.slice(current.length + 1);
    if (chosenId && !wasId) void tick().then(() => detailEl?.focus({ preventScroll: true }));
    else if (!chosenId && wasId) void listRef?.focusOn(wasId);
  });

  function pick(id: string, open: boolean) {
    onselect?.(current, id, open && phone);
  }

  /** Merged or discarded: say so, re-read, and the next open draft takes the detail. */
  async function closed(message: string) {
    toaster.show({ message, tone: 'ok' });
    await Promise.all([openList.load(), closedList.load()]);
    void store.refresh();
    leaveDetail();
  }

  // New draft: a name, then the draft opens (empty; Add a change stages into it).
  let newOpen = $state(false);
  let newTitle = $state('');
  let newBusy = $state(false);
  let newError = $state('');
  async function create(event?: SubmitEvent) {
    event?.preventDefault();
    newBusy = true;
    const title = newTitle.trim();
    const made = await send((c) => c.post<DraftWritten>(DRAFTS_PATH, title ? { title } : {}));
    newBusy = false;
    if (!made.ok) {
      newError = made.message;
      return;
    }
    newOpen = false;
    newTitle = '';
    toaster.show({ message: `${title || 'Untitled draft'} started; nothing is live until it is merged.`, tone: 'ok' });
    await openList.load();
    onselect?.('open', made.value.id, phone);
  }
  function askNew() {
    newTitle = '';
    newError = '';
    newOpen = true;
  }

  // Discard…: names the draft and what is lost; nothing was ever live.
  let discarding = $state<DraftDetail | null>(null);
  let discardBusy = $state(false);
  let discardError = $state('');
  async function discard() {
    const d = discarding;
    if (!d) return;
    discardBusy = true;
    const done = await send((c) => c.delete<DraftWritten>(draftPath(d.id, `?version=${d.version}`)));
    discardBusy = false;
    if (!done.ok) {
      discardError = done.message;
      return;
    }
    discarding = null;
    await closed(done.value.message);
  }
</script>

<!-- The phone frame has the top bar's title (B_PhoneDrafts): the page line is for screen readers only. -->
<PageLine title={openList.data ? 'Drafts' : ''} class={chrome?.phone ? 'pageline--echo' : ''}>
  <h1>{#if openCount !== null}<span class="pageline__num">{openCount}</span> open{:else}Drafts{/if}</h1>
  <p class="pageline__context">staged changes, live only after Merge</p>
</PageLine>

{#snippet newButton(where: 'bar' | 'note')}
  <button class={where === 'bar' ? 'btn drafts__newbtn' : 'btn btn--primary drafts__notebtn'} type="button" onclick={askNew}><Icon name="plus" />New draft</button>
{/snippet}

<!-- On the phone frame the tabs and New draft are one row on the ground (B_PhoneDrafts): the head is that row. -->
<section data-fid={compact ? undefined : 'window'} class="card tabs drafts window-fill" class:drafts--compact={compact} aria-label="Drafts">
  <div class="card__head tabs__strip" data-fid={chrome?.phone ? 'window-tabs' : 'window-bar'}>
    <div class="tabs__tabs" role="tablist" aria-label="Drafts" data-fid={chrome?.phone ? undefined : 'window-tabs'}>
      {#each TABS as t, index (t.id)}
        <button
          type="button"
          role="tab"
          class="tabs__tab"
          id="{uid}-tab-{t.id}"
          aria-selected={current === t.id}
          aria-controls="{uid}-panel"
          tabindex={current === t.id ? 0 : -1}
          bind:this={tabEls[t.id]}
          onclick={() => onselect?.(t.id, '', false)}
          onkeydown={(event) => tabKey(event, index)}
          >{t.label}{#if t.id === 'open' && openCount !== null}<span class="tabs__count">{openCount}</span>{/if}</button
        >
      {/each}
    </div>
    {#if openList.data}<div class="drafts__new" data-fid="drafts-new">{@render newButton('bar')}</div>{/if}
  </div>
  {#if phone && current === 'open' && items.length && !chosenId}
    <p class="drafts__live drafts__live--list" data-fid="drafts-live-note"><Icon name="eye-off" /><span><b>Not live.</b> No pings, cards or reminders until you merge.</span></p>
  {/if}
  <div
    class="drafts__body"
    class:drafts__body--empty={list.data && !items.length}
    class:drafts__body--waiting={!list.data}
    role="tabpanel"
    id="{uid}-panel"
    aria-labelledby="{uid}-tab-{current}"
  >
    {#if list.error && !list.data}
      <LoadError thing="drafts" reason="{list.error} Your drafts are kept on the server; nothing was lost." onretry={() => void list.load()} />
    {:else if !list.data}
      {#if !phone}<div class="drafts__list drafts__list--waiting" data-fid="drafts-list" aria-busy="true"></div>{/if}
      <div class="state-pane drafts__loading" data-fid="state-pane"><div class="drafts__loader" data-fid="state-loading"><LoadingState text="Loading drafts…" /></div></div>
    {:else if !items.length}
      <div class="inbox-empty" data-fid={chrome?.phone ? undefined : 'empty-body'}>
        {#snippet ways()}
          {@render newButton('note')}
          <a class="btn drafts__notebtn" href="/history">Open History</a>
        {/snippet}
        <StateNote icon="layers" title={current === 'open' ? 'No open drafts' : 'No closed drafts'} actions={current === 'open' ? ways : undefined}>
          {#if current === 'open'}Stage several schedule changes, check the preview, then merge them as one change. Nothing in a draft is live until you merge it.
          {:else}Merged, discarded and expired drafts show here.{/if}
        </StateNote>
        {#if current === 'open' && closedList.data?.drafts.length}
          <section class="inbox-empty__recent" data-fid="empty-recent" aria-labelledby="{uid}-recent">
            <h3 class="cap inbox-empty__head" id="{uid}-recent" data-fid="empty-recent-head">Recently closed</h3>
            <ul class="inbox-empty__rows">
              {#each closedList.data.drafts.slice(0, 3) as d (d.id)}
                <li>
                  <a class="inbox-empty__row" data-fid="empty-recent-row" href="/drafts?tab=closed&item={encodeURIComponent(d.id)}">
                    <span class="inbox__art drafts__art drafts__art--sm" aria-hidden="true">{#if portraits[d.id]}<Portrait boss={portraits[d.id]!} size="md" />{/if}</span>
                    <span class="inbox-empty__lines">
                      <span class="inbox-empty__what">{d.title}</span>
                      <span class="inbox-empty__when">{closedLabel(d)} · <span class="mono">{shortAt(d.updated_at, timeZone)}</span></span>
                    </span>
                  </a>
                </li>
              {/each}
            </ul>
          </section>
        {/if}
      </div>
    {:else}
      <div class="drafts__list" data-fid="drafts-list" hidden={phone && Boolean(chosenId)}>
        <DraftList
          bind:this={listRef}
          {items}
          selected={chosenId ?? ''}
          label={current === 'open' ? 'Open drafts' : 'Closed drafts'}
          follow={!phone}
          {portraits}
          {timeZone}
          now={store.week?.generated_at ?? ''}
          onpick={pick}
          fresh={openList.fresh}
        />
      </div>
      <div class="drafts__detail" hidden={!chosenId} tabindex="-1" bind:this={detailEl} {@attach enter(chosenId)}>
        {#if chosen}
          {#key chosen.id}
            <DraftView
              id={chosen.id}
              week={store.week}
              {timeZone}
              bar={phone}
              {toaster}
              onchanged={() => void Promise.all([openList.refresh(), closedList.refresh()])}
              onclosed={(message) => void closed(message)}
              onadd={(d) => onadd?.({ id: d.id, title: d.title, version: d.version })}
              ondiscard={(d) => {
                discardError = '';
                discarding = d;
              }}
            />
          {/key}
        {/if}
      </div>
    {/if}
  </div>
</section>

<Modal bind:open={() => discarding !== null, (value) => !value && (discarding = null)} title={discarding ? `Discard ${discarding.title}?` : 'Discard'} eyebrow="Drafts" narrow>
  <p>Its {discarding?.ops.length ?? 0} staged change{discarding?.ops.length === 1 ? '' : 's'} go with it. Nothing was live, so nothing on the schedule changes.</p>
  <p class="field__error" role="alert">{discardError}</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Keep it</button>
    <button class="btn btn--danger" type="button" disabled={discardBusy} onclick={() => void discard()}><PendingLabel pending={discardBusy} label="Discarding…">Discard draft</PendingLabel></button>
  {/snippet}
</Modal>

<Modal bind:open={newOpen} title="New draft" eyebrow="Drafts" narrow>
  <form id="{uid}-new" onsubmit={create}>
    <label class="field">
      <span>Name</span>
      <input bind:value={newTitle} maxlength="200" placeholder="Name, e.g. Wednesday shuffle" aria-describedby="{uid}-new-err" />
    </label>
    <p class="note">Leave it empty and the draft is named after its changes.</p>
    <p class="field__error" id="{uid}-new-err" role="alert">{newError}</p>
  </form>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Cancel</button>
    <button class="btn btn--primary" type="submit" form="{uid}-new" disabled={newBusy}><PendingLabel pending={newBusy} label="Starting…"><Icon name="plus" />Start draft</PendingLabel></button>
  {/snippet}
</Modal>
