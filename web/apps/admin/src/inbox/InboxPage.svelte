<!--
  The Inbox (v4 inbox.html, redesigned by user request 2026-09-25): one window
  with "Extractor" and "Self-service" title-bar tabs, a compact listbox of the
  tab's items beside the selected item's detail (the Config side-list
  pattern). A read-only "Past" tab lists closed items the same way, loaded
  on first open. Deep links: ?tab=&item=. Phones show the list, then the
  detail with a back action — one scrolling panel either way.
-->
<script lang="ts">
  import PageLine from '../shell/PageLine.svelte';
  import '@kanade/ui/styles/panes.scss';
  import '@kanade/ui/styles/evidence.scss';
  import '@kanade/ui/styles/inbox.scss';
  import type { ApproveRequest, InboxTab, Proposal } from '@kanade/api-types';
  import { Icon, LoadError, LoadingState, Modal, PendingLabel, Toaster, enter } from '@kanade/ui';
  import { tick, untrack } from 'svelte';
  import { Resource, send } from '../resource.svelte';
  import type { AdminWeek } from '../store.svelte';
  import { parseEdit } from './edit';
  import { isProposal, REASON_MAX, reasonProblem, refusalText, title } from './flags';
  import InboxDetail from './InboxDetail.svelte';
  import InboxEmpty from './InboxEmpty.svelte';
  import InboxList from './InboxList.svelte';
  import PastDetail from './PastDetail.svelte';
  import PastList from './PastList.svelte';
  import { PastLog } from './pastLog.svelte';
  import { getChrome } from '../shell/chrome';

  type PageTab = InboxTab | 'past';

  let {
    store,
    toaster,
    tab = '',
    item = '',
    onselect,
  }: {
    store: AdminWeek;
    toaster: Toaster;
    tab?: string;
    item?: string;
    /** Pushes a history entry when `open` (phones: Back returns to the list). */
    onselect?: (tab: PageTab, item: string, open: boolean) => void;
  } = $props();

  const inbox = new Resource<Proposal[]>('/api/admin/inbox');
  $effect(() => void inbox.load());

  // Past (read-only, closed items) has no live count and loads when first opened.
  const TABS: { id: PageTab; label: string }[] = [
    { id: 'extractor', label: 'Extractor' },
    { id: 'self_service', label: 'Self-service' },
    { id: 'past', label: 'Past' },
  ];
  const uid = $props.id();
  const current = $derived<PageTab>(tab === 'self_service' || tab === 'past' ? tab : 'extractor');
  const isPast = $derived(current === 'past');
  const items = $derived(isPast ? [] : (inbox.data ?? []).filter((p) => p.tab === current));
  const counts = $derived(Object.fromEntries(TABS.map((t) => [t.id, (inbox.data ?? []).filter((p) => p.tab === t.id).length])));

  let phone = $state(false);
  $effect(() => {
    const query = window.matchMedia('(max-width: 899px)');
    const update = () => (phone = query.matches);
    update();
    query.addEventListener('change', update);
    return () => query.removeEventListener('change', update);
  });
  // Wide screens always show a detail (the first item by default); phones
  // show the list until an item is picked.
  const chosen = $derived(items.find((p) => p.id === item) ?? (phone ? undefined : items[0]));

  const past = new PastLog();
  // Each visit to the tab retries a failed first load; a loaded list is kept.
  $effect(() => {
    if (!isPast) return;
    untrack(() => {
      if (past.items === null) void past.load();
    });
  });
  let moreNote = $state('');
  async function loadMore() {
    const before = past.items?.length ?? 0;
    await past.load(true);
    const added = (past.items?.length ?? 0) - before;
    if (added <= 0) return;
    moreNote = `${added} older item${added === 1 ? '' : 's'} loaded.`;
    // On the last page the button goes away: focus returns to the list, not the page.
    if (!past.next) void list?.focusOn(past.items![before]!.id);
  }
  const pastItems = $derived(past.items ?? []);
  const chosenPast = $derived(isPast ? (pastItems.find((p) => p.id === item) ?? (phone ? undefined : pastItems[0])) : undefined);
  const timeZone = $derived(store.week?.timezone ?? 'Asia/Kuala_Lumpur');
  // A primitive key for the open item: a refreshed copy of the same item must not replay the enter.
  const chosenId = $derived((isPast ? chosenPast?.id : chosen?.id) ?? null);

  // The phone frame's open item (B_PhoneInbox): no page line, tabs or window
  // chrome; "‹ Inbox" in the top bar; the decision as a bottom action bar.
  const chrome = getChrome();
  const compact = $derived(phone && Boolean(chosenId) && Boolean(chrome?.phone));
  $effect(() => {
    if (!compact || !chrome) return;
    chrome.back({ label: 'Inbox', name: 'Back to the list (Inbox)', go: () => leaveDetail() });
    return () => chrome.back(null);
  });

  let busy = $state(false);
  let error = $state('');
  let rejectOpen = $state(false);
  let rejecting = $state(false);
  let reason = $state('');
  let reasonError = $state('');
  const tabEls: Record<string, HTMLButtonElement> = {};

  function pickTab(id: PageTab) {
    error = '';
    onselect?.(id, '', false);
  }

  function tabKey(event: KeyboardEvent, index: number) {
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const next = TABS[(target + TABS.length) % TABS.length]!;
    pickTab(next.id);
    tabEls[next.id]?.focus();
  }

  // Whether the phone's detail entry came from a pick here (so leaving it pops
  // it) or a deep link; read from the entry itself so Forward/Back agree.
  const pushedHere = () => (history.state as { inboxDetail?: boolean } | null)?.inboxDetail === true;
  // Phones swap list and detail: the detail comes in forward, and the list
  // comes back backward when the detail closes (by Back or history).
  let returns = $state(0);
  let detailWasOpen = false;
  $effect(() => {
    const open = phone && Boolean(chosenId);
    if (detailWasOpen && !open && phone) untrack(() => returns++);
    detailWasOpen = open;
  });

  function leaveDetail() {
    if (phone && pushedHere()) history.back();
    else onselect?.(current, '', false);
  }

  let list = $state<{ focusOn: (id: string) => Promise<void> }>();
  let detailEl = $state<HTMLDivElement>();
  // Phones hide the list while a detail is open: focus follows the swap both
  // ways (into the detail after a pick or Forward, back to the opened option
  // on return). `restore` names a different option to land on (after a decision).
  let restore = '';
  let shown = '';
  $effect(() => {
    const key = `${current}/${chosenId ?? ''}`;
    const was = shown;
    shown = key;
    if (!phone || key === was || !was.startsWith(`${current}/`)) return;
    const wasId = was.slice(current.length + 1);
    if (chosenId && !wasId) {
      void tick().then(() => detailEl?.focus({ preventScroll: true }));
    } else if (!chosenId && wasId) {
      void list?.focusOn(restore || wasId);
      restore = '';
    }
  });

  function pick(id: string, open: boolean) {
    error = '';
    restore = '';
    onselect?.(current, id, open && phone);
  }

  async function after(message: string) {
    toaster.show({ message, tone: 'ok' });
    // Before the reload: its effects run inside `load()` and consume `restore`.
    const index = items.findIndex((p) => p.id === chosen?.id);
    restore = phone && index >= 0 ? ((items[index + 1] ?? items[index - 1])?.id ?? '') : '';
    await inbox.load();
    void store.refresh();
    await tick();
    restore = '';
    // The next item of the tab takes the detail (or, on a phone, the list returns with it active).
    leaveDetail();
  }

  /** Words for a refusal; learns the Discord-only rule and re-reads what moved. */
  function refused(code: string | null | undefined, message: string): string {
    if (code === 'discord_session_required') store.proposalsLocked = true;
    if (code === 'stale' || code === 'expired') void inbox.load();
    return refusalText(code, message);
  }

  async function approve(p: Proposal, body: ApproveRequest) {
    error = '';
    busy = true;
    const result = await send((c) => c.post<{ message: string }>(`/api/admin/inbox/${encodeURIComponent(p.id)}/approve`, body));
    busy = false;
    if (result.ok) await after(result.value.message);
    else error = refused(result.code, result.message);
  }

  function move(p: Proposal, text: string) {
    // The proposal's own boss week (the reset weekday is the same every week).
    const edit = parseEdit(text, p, store.week?.days[0]?.dow ?? store.week?.reset ?? 'Thu');
    if (!edit.ok) {
      error = edit.message;
      return;
    }
    void approve(p, { version: p.version, day: edit.day, time: edit.time });
  }

  async function reject() {
    const p = chosen;
    if (!p) return;
    reasonError = reasonProblem(p, reason);
    if (reasonError) return;
    // Proposals take no reason: nothing would keep it.
    const text = isProposal(p) ? '' : reason.trim();
    rejecting = true;
    const result = await send((c) =>
      c.post<{ message: string }>(`/api/admin/inbox/${encodeURIComponent(p.id)}/reject`, { version: p.version, ...(text ? { reason: text } : {}) }),
    );
    rejecting = false;
    if (!result.ok) {
      reasonError = refused(result.code, result.message);
      return;
    }
    rejectOpen = false;
    reason = '';
    await after(result.value.message);
  }
</script>

<PageLine title={inbox.data ? 'Inbox' : ''} class={compact ? 'pageline--echo' : ''}>
  <h1>{#if inbox.data}<span class="pageline__num">{inbox.data.length}</span> change{inbox.data.length === 1 ? '' : 's'} waiting{:else}Inbox{/if}</h1>
</PageLine>

<section data-fid="window" class="card tabs inbox window-fill" class:inbox--compact={compact} aria-label="Inbox">
  <div class="card__head tabs__strip" data-fid="window-bar">
    <div class="tabs__tabs" role="tablist" aria-label="Inbox" data-fid="window-tabs">
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
          onclick={() => pickTab(t.id)}
          onkeydown={(event) => tabKey(event, index)}
          >{t.label}{#if t.id !== 'past'}<span class="tabs__count">{counts[t.id] ?? 0}</span>{/if}</button
        >
      {/each}
    </div>
  </div>
  <div
    class="inbox__body"
    class:inbox__body--empty={isPast ? past.items && !past.items.length : inbox.data && !items.length}
    role="tabpanel"
    id="{uid}-panel"
    aria-labelledby="{uid}-tab-{current}"
  >
    {#if isPast}
      {#if past.items === null && past.error}
        <LoadError thing="past decisions" reason={past.error} onretry={() => void past.load()} />
      {:else if past.items === null}
        <LoadingState text="Loading past decisions…" />
      {:else}
        <div class="inbox__list" data-fid="inbox-list" hidden={phone && Boolean(chosenPast)} {@attach enter(returns || null, 'backward')}>
          <PastList bind:this={list} items={pastItems} selected={chosenPast?.id ?? ''} follow={!phone} {timeZone} onpick={pick} />
          {#if past.error}<p class="flash flash--error past__more-error" role="alert">{past.error}</p>{/if}
          {#if past.next}
            <div class="past__more">
              <button type="button" class="btn" disabled={past.loading} onclick={() => void loadMore()}
                ><PendingLabel pending={past.loading} label="Loading…">Load older items</PendingLabel></button
              >
            </div>
          {/if}
          <p class="vh" aria-live="polite">{moreNote}</p>
        </div>
        <div class="inbox__detail" hidden={!chosenPast} tabindex="-1" bind:this={detailEl} {@attach enter(chosenId)}>
          {#if chosenPast}
            {#if phone && !compact}
              <button type="button" class="btn btn--ghost inbox__back" onclick={leaveDetail}>
                <span aria-hidden="true">←</span> Back to the list
              </button>
            {/if}
            {#key chosenPast.id}
              <PastDetail item={chosenPast} {timeZone} />
            {/key}
          {/if}
        </div>
      {/if}
    {:else if inbox.error}
      <LoadError thing="the inbox" reason={inbox.error} onretry={() => void inbox.load()} />
    {:else if !inbox.data}
      <LoadingState text="Loading the inbox…" />
    {:else if !items.length}
      {#key current}<InboxEmpty tab={current === 'self_service' ? 'self_service' : 'extractor'} {timeZone} />{/key}
    {:else}
      <div class="inbox__list" data-fid="inbox-list" hidden={phone && Boolean(chosen)} {@attach enter(returns || null, 'backward')}>
        <InboxList
          bind:this={list}
          {items}
          selected={chosen?.id ?? ''}
          label="{current === 'extractor' ? 'Extractor' : 'Self-service'} items"
          follow={!phone}
          onpick={pick}
        />
      </div>
      <div class="inbox__detail" hidden={!chosen} tabindex="-1" bind:this={detailEl} {@attach enter(chosenId)}>
        {#if chosen}
          {#if phone && !compact}
            <button type="button" class="btn btn--ghost inbox__back" onclick={leaveDetail}>
              <span aria-hidden="true">←</span> Back to the list
            </button>
          {/if}
          {#key chosen.id}
            <InboxDetail
              p={chosen}
              bar={phone}
              now={store.week?.generated_at ?? ''}
              {timeZone}
              {busy}
              locked={store.proposalsLocked}
              {error}
              onapprove={(body) => void approve(chosen!, body)}
              onmove={(text) => move(chosen!, text)}
              onreject={() => {
                reason = '';
                reasonError = '';
                rejectOpen = true;
              }}
            />
          {/key}
        {/if}
      </div>
    {/if}
  </div>
</section>

<Modal bind:open={rejectOpen} title={chosen ? `Reject ${title(chosen)}?` : 'Reject'} eyebrow="Inbox" narrow>
  <p>Nothing on the schedule changes. {chosen?.self_service ? 'The member is told, with your reason.' : 'The card in Discord is marked rejected.'}</p>
  {#if chosen && !isProposal(chosen)}
    <label class="field">
      <span>Reason</span>
      <textarea
        bind:value={reason}
        rows="3"
        maxlength={REASON_MAX}
        aria-invalid={reasonError ? 'true' : undefined}
        aria-describedby="{uid}-reason-help {uid}-reason-err"
        required
      ></textarea>
    </label>
    <p class="note" id="{uid}-reason-help"><span class="mono">{[...reason].length}/{REASON_MAX}</span></p>
  {/if}
  <p class="field__error" id="{uid}-reason-err" role="alert">{reasonError}</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Keep it</button>
    <button class="btn btn--primary" type="button" onclick={() => void reject()}
      ><PendingLabel pending={rejecting} label="Rejecting…"><Icon name="x" /> Reject change</PendingLabel></button
    >
  {/snippet}
</Modal>
