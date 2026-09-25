<!--
  The Inbox (v4 inbox.html, redesigned by user request 2026-09-25): one window
  with "Extractor" and "Self-service" title-bar tabs, a compact listbox of the
  tab's items beside the selected item's detail (the Config side-list
  pattern). Deep links: ?tab=&item=. Phones show the list, then the detail
  with a back action — one scrolling panel either way.
-->
<script lang="ts">
  import '@kanade/ui/styles/panes.scss';
  import '@kanade/ui/styles/evidence.scss';
  import '@kanade/ui/styles/inbox.scss';
  import type { ApproveRequest, InboxTab, Proposal } from '@kanade/api-types';
  import { Icon, Modal, Toaster } from '@kanade/ui';
  import { tick } from 'svelte';
  import { Resource, send } from '../resource.svelte';
  import { parseWhen } from '../sheet/parseWhen';
  import type { AdminWeek } from '../store.svelte';
  import { REASON_MAX, reasonProblem, title } from './flags';
  import InboxDetail from './InboxDetail.svelte';
  import InboxList from './InboxList.svelte';

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
    onselect?: (tab: InboxTab, item: string, open: boolean) => void;
  } = $props();

  const inbox = new Resource<Proposal[]>('/api/admin/inbox');
  $effect(() => void inbox.load());

  const TABS: { id: InboxTab; label: string }[] = [
    { id: 'extractor', label: 'Extractor' },
    { id: 'self_service', label: 'Self-service' },
  ];
  const uid = $props.id();
  const current = $derived<InboxTab>(tab === 'self_service' ? 'self_service' : 'extractor');
  const items = $derived((inbox.data ?? []).filter((p) => p.tab === current));
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

  let busy = $state(false);
  let error = $state('');
  let rejectOpen = $state(false);
  let reason = $state('');
  let reasonError = $state('');
  const tabEls: Record<string, HTMLButtonElement> = {};

  function pickTab(id: InboxTab) {
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
  function leaveDetail() {
    if (phone && pushedHere()) history.back();
    else onselect?.(current, '', false);
  }

  let list = $state<ReturnType<typeof InboxList>>();
  let detailEl = $state<HTMLDivElement>();
  // Phones hide the list while a detail is open: focus follows the swap both
  // ways (into the detail after a pick, back to the opened option on return).
  let focusDetail = false;
  let restore = '';
  let shown = '';
  $effect(() => {
    const key = `${current}/${chosen?.id ?? ''}`;
    const was = shown;
    shown = key;
    if (!phone || key === was || !was.startsWith(`${current}/`)) return;
    const wasId = was.slice(current.length + 1);
    if (chosen && focusDetail) {
      focusDetail = false;
      void tick().then(() => detailEl?.focus());
    } else if (!chosen && wasId) {
      void list?.focusOn(restore || wasId);
      restore = '';
    }
  });

  function pick(id: string, open: boolean) {
    error = '';
    focusDetail = open && phone;
    onselect?.(current, id, open && phone);
  }

  async function after(message: string) {
    toaster.show({ message, tone: 'ok' });
    const index = items.findIndex((p) => p.id === chosen?.id);
    await inbox.load();
    void store.refresh();
    // The next item of the tab takes the detail (or, on a phone, the list returns with it active).
    if (phone) restore = (items[index] ?? items[index - 1])?.id ?? '';
    leaveDetail();
  }

  async function approve(p: Proposal, body: ApproveRequest) {
    error = '';
    busy = true;
    const result = await send((c) => c.post<{ message: string }>(`/api/admin/inbox/${encodeURIComponent(p.id)}/approve`, body));
    busy = false;
    if (result.ok) await after(result.value.message);
    else error = result.message;
  }

  function move(p: Proposal, text: string, force: boolean) {
    const run = p.run_id ? store.run(p.run_id) : undefined;
    const parsed = parseWhen(text, store.week?.days ?? [], { day: run?.day ?? 0, time: run?.time ?? null });
    if (!parsed.ok) {
      error = parsed.message;
      return;
    }
    void approve(p, { version: p.version, day: parsed.slot.day, time: parsed.slot.time, force });
  }

  async function reject() {
    const p = chosen;
    if (!p) return;
    reasonError = reasonProblem(p, reason);
    if (reasonError) return;
    const text = reason.trim();
    const result = await send((c) =>
      c.post<{ message: string }>(`/api/admin/inbox/${encodeURIComponent(p.id)}/reject`, { version: p.version, ...(text ? { reason: text } : {}) }),
    );
    if (!result.ok) {
      reasonError = result.message;
      return;
    }
    rejectOpen = false;
    reason = '';
    await after(result.value.message);
  }
</script>

<div class="page-head">
  <div>
    <p class="eyebrow">From the party chat and the members</p>
    <h1>{inbox.data ? `${inbox.data.length} change${inbox.data.length === 1 ? '' : 's'} waiting` : 'Inbox'}</h1>
    <p class="note">Approving here is the same as reacting ✅ on the card in Discord: it applies the change and edits the card.</p>
  </div>
</div>

<section class="card tabs inbox window-fill" aria-label="Inbox">
  <div class="card__head tabs__strip">
    <div class="tabs__tabs" role="tablist" aria-label="Inbox">
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
          >{t.label}<span class="tabs__count">{counts[t.id] ?? 0}</span></button
        >
      {/each}
    </div>
  </div>
  <div class="inbox__body" class:inbox__body--empty={inbox.data && !items.length} role="tabpanel" id="{uid}-panel" aria-labelledby="{uid}-tab-{current}">
    {#if inbox.error}
      <p class="flash flash--error" role="alert">{inbox.error}</p>
    {:else if !inbox.data}
      <p class="note" aria-busy="true">Loading the inbox…</p>
    {:else}
      <div class="inbox__list" hidden={phone && Boolean(chosen)}>
        <InboxList
          bind:this={list}
          {items}
          selected={chosen?.id ?? ''}
          label="{current === 'extractor' ? 'Extractor' : 'Self-service'} items"
          follow={!phone}
          empty={current === 'extractor' ? 'The extractor posts a card when it reads a change in a watched channel.' : 'Members’ requests arrive here.'}
          onpick={pick}
        />
      </div>
      <div class="inbox__detail" hidden={!chosen} tabindex="-1" bind:this={detailEl}>
        {#if chosen}
          {#if phone}
            <button type="button" class="btn btn--ghost inbox__back" onclick={leaveDetail}>
              <span aria-hidden="true">←</span> Back to the list
            </button>
          {/if}
          {#key chosen.id}
            <InboxDetail
              p={chosen}
              {busy}
              {error}
              onapprove={(body) => void approve(chosen!, body)}
              onmove={(text, force) => move(chosen!, text, force)}
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
  <label class="field">
    <span>Reason{chosen?.tab === 'self_service' ? '' : ' (optional)'}</span>
    <textarea
      bind:value={reason}
      rows="3"
      maxlength={REASON_MAX}
      aria-invalid={reasonError ? 'true' : undefined}
      aria-describedby="{uid}-reason-help {uid}-reason-err"
      required={chosen?.tab === 'self_service'}
    ></textarea>
  </label>
  <p class="note" id="{uid}-reason-help"><span class="mono">{[...reason].length}/{REASON_MAX}</span></p>
  <p class="field__error" id="{uid}-reason-err" role="alert">{reasonError}</p>
  {#snippet footer(close)}
    <button class="btn" type="button" onclick={close}>Keep it</button>
    <button class="btn btn--primary" type="button" onclick={() => void reject()}><Icon name="x" /> Reject change</button>
  {/snippet}
</Modal>
