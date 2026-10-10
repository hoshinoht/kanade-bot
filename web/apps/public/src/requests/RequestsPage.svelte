<!--
  My requests (`/requests?open=<id>`; boards Requests, Requests-Expired,
  PhoneRequests): the member's own requests, newest first, each with its
  state chip (waiting outlined and dashed, approved and declined washed,
  withdrawn and expired plain), beside the open one: Sent → Admin review →
  Decided, what holds until an admin decides, the note, Withdraw… while it
  waits, the admin's reason when declined, a link to what changed when
  approved, and "Ask again" when it expired. On phones the open row unfolds
  in place. "Ask for a change…" opens the form.
-->
<script lang="ts">
  import type { MemberRequest } from '@kanade/api-types';
  import { Icon, LoadingState, Modal, StateNote, type Toaster } from '@kanade/ui';
  import { tick, untrack, type Snippet } from 'svelte';
  import type { Route } from '../route.svelte';
  import { tokenWords, whenWords } from '../timings/ownership';
  import type { MemberTimingsList } from '../timings/timings.svelte';
  import type { MemberWeeks } from '../weeks.svelte';
  import { follow } from '../writes/follow';
  import { counterWords, headline, instantWords, KIND_NOUNS, proposalWords, rowTail, runWhen, STATE_TONES, STATE_WORDS, stepsOf, waitingWords, weekly } from './form';
  import type { MemberRequestsList } from './requests.svelte';
  import './requests.scss';

  let {
    requests,
    weeks,
    timings,
    route,
    toaster,
    phone,
    timeZone,
    notice,
  }: {
    requests: MemberRequestsList;
    weeks: MemberWeeks;
    /** The member's weekly timings: what a weekly change changes from. */
    timings: MemberTimingsList;
    route: Route;
    toaster: Toaster;
    phone: boolean;
    /** The guild's zone (the week's), for when a request was sent and decided. */
    timeZone: string;
    notice?: Snippet;
  } = $props();

  const uid = $props.id();

  $effect(() =>
    untrack(() => {
      void requests.load();
      if (!timings.data) void timings.load();
    }),
  );

  const data = $derived(requests.data);
  const list = $derived(data?.requests ?? []);
  const now = $derived(data?.generated_at ?? new Date().toISOString());
  // Wide screens always show one request (the first by default); phones unfold only the one asked for.
  const openId = $derived(route.params.get('open') ?? (phone ? '' : (list[0]?.id ?? '')));
  const open = $derived(list.find((r) => r.id === openId) ?? null);
  const waitingCount = $derived(list.filter((r) => r.state === 'waiting').length);
  const decidedCount = $derived(list.filter((r) => r.state === 'approved' || r.state === 'rejected').length);

  const titleOf = (r: MemberRequest) => headline(r.kind, tokenWords({ bosses: r.bosses.length ? r.bosses : (r.run?.bosses ?? []) }));

  /** When the subject happens: the run's day and time, or the weekly timing it proposes. */
  function whenOf(r: MemberRequest): string {
    const p = r.proposed;
    if (r.kind === 'change_fixed') return proposalWords(r, timings.data?.timings.find((t) => t.id === r.fixed_id) ?? null);
    if (r.kind === 'new_fixed') return p && p.day !== null && p.time !== null ? weekly(p.day, p.time) : '';
    if (!r.run) return '';
    const found = weeks.find(r.run.id);
    return found ? runWhen(found.run, found.week) : (r.run.time ?? 'own time');
  }

  /** Where an approved request's change shows: the run on the Week, or the weekly timings. */
  function changedAt(r: MemberRequest): { href: string; label: string } | null {
    if (r.kind === 'new_fixed' || r.kind === 'change_fixed') return { href: '/mine?week=timings', label: 'See your weekly timings' };
    if (!r.run) return null;
    const found = weeks.find(r.run.id);
    if (!found) return { href: `/runs/${encodeURIComponent(r.run.id)}`, label: 'Open the run' };
    return { href: `/?${new URLSearchParams({ run: r.run.id, ...(found.which === 'next' ? { week: 'next' } : {}) })}`, label: 'Open the run' };
  }

  /** An expired request's "Ask again": this week's run of the same weekly timing, when there is one. */
  function againOf(r: MemberRequest): { href: string; hint: string } {
    const run = r.fixed_id ? weeks.this?.runs.find((x) => x.fixed_id === r.fixed_id && x.status !== 'done' && x.status !== 'cancelled') : undefined;
    if (run && weeks.this) return { href: `/requests/new?${new URLSearchParams({ run: run.id, kind: r.kind })}`, hint: `${tokenWords(run)} runs again ${runWhen(run, weeks.this)}.` };
    return { href: `/requests/new?${new URLSearchParams({ kind: r.kind })}`, hint: '' };
  }

  function show(event: MouseEvent, id: string) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    route.set({ open: phone && id === openId ? '' : id });
  }

  // Withdraw…: a confirm, then the write (no fresh sign-in needed).
  let withdrawing = $state<MemberRequest | null>(null);
  let confirmOpen = $state(false);
  let withdrawKey = $state<HTMLButtonElement>();

  async function withdraw() {
    const r = withdrawing;
    if (!r) return;
    confirmOpen = false;
    const outcome = await requests.withdraw(r.id);
    if (!outcome) return;
    if (outcome.kind === 'done') toaster.show({ message: `Withdrawn: ${titleOf(r)}. Nothing changed.`, tone: 'ok' });
    else toaster.show({ message: `Couldn't withdraw it: ${outcome.kind === 'refused' || outcome.kind === 'limit' ? outcome.words : 'try again.'}`, tone: 'error' });
    await tick();
    document.querySelector<HTMLElement>(`[data-request="${CSS.escape(r.id)}"]`)?.focus({ preventScroll: true });
  }
</script>

{#snippet chip(r: MemberRequest)}
  <span class="status-chip req-chip req-chip--{r.state} {STATE_TONES[r.state] ? `status-chip--${STATE_TONES[r.state]}` : ''}">{STATE_WORDS[r.state]}</span>
{/snippet}

{#snippet steps(r: MemberRequest)}
  {@const s = stepsOf(r.state)}
  <div class="req-progress">
    <div class="req-steps" aria-hidden="true" data-fid="requests-steps">
      <span class="req-steps__dot req-steps__dot--done">1</span><span class="req-steps__bar req-steps__bar--done"></span><span class="req-steps__dot req-steps__dot--done">2</span><span
        class="req-steps__bar"
        class:req-steps__bar--done={s.reviewed}
      ></span><span class="req-steps__dot req-steps__dot--{s.last}">{s.last === 'gone' ? '–' : '3'}</span>
    </div>
    <div class="req-steps__labels" aria-hidden="true"><span>Sent</span><span>Admin review</span><span>{s.label}</span></div>
    <p class="vh">{s.words}</p>
  </div>
{/snippet}

{#snippet withdrawButton(r: MemberRequest, full: boolean)}
  <button
    type="button"
    class="btn btn--danger"
    class:btn--key={!full}
    class:btn--full={full}
    disabled={requests.busy}
    bind:this={withdrawKey}
    onclick={() => {
      withdrawing = r;
      confirmOpen = true;
    }}>Withdraw…</button
  >
{/snippet}

{#snippet detail(r: MemberRequest, head: boolean)}
  {@const when = whenOf(r)}
  {@const changed = r.state === 'approved' ? changedAt(r) : null}
  {#if head}
    <div class="req-detail__head">
      <p class="cap">{KIND_NOUNS[r.kind]} · sent {instantWords(r.sent_at, timeZone, now)}</p>
      <h2 class="req-detail__title" id="{uid}-detail">{titleOf(r)}{when ? ` · ${when}` : ''}</h2>
      <span class="req-detail__chips">{@render chip(r)}{#if r.channel}<span class="field__hint">{r.channel}</span>{/if}</span>
    </div>
  {/if}
  {@render steps(r)}
  {#if r.state === 'waiting'}
    {@const timing = timings.data?.timings.find((t) => t.id === r.fixed_id)}
    <p class="infobox">
      <Icon name="info" /><span>{waitingWords(r.kind, timing ? whenWords(timing) : '', r.expires_at ? instantWords(r.expires_at, timeZone, now) : '')}</span>
    </p>
  {:else if r.state === 'approved'}
    <p class="flash flash--ok"><Icon name="check" /><span><b>Approved{r.decided_by ? ` by ${r.decided_by}` : ''}</b>{r.decided_at ? ` ${instantWords(r.decided_at, timeZone, now)}` : ''}. The change is made.</span></p>
  {:else if r.state === 'rejected'}
    <p class="flash flash--error">
      <Icon name="x" /><span><b>Declined{r.decided_by ? ` by ${r.decided_by}` : ''}</b>{r.decided_at ? ` ${instantWords(r.decided_at, timeZone, now)}` : ''}.{r.reason ? ` “${r.reason}”` : ''} Nothing changed.</span>
    </p>
  {:else if r.state === 'expired'}
    <p class="flash flash--warn">
      <Icon name="clock" /><span
        ><b>No admin decided before the boss week ended,</b> so this request expired{r.expires_at ? ` on ${instantWords(r.expires_at, timeZone, now, false)}` : ''} at reset. Nothing
        changed, and it no longer counts toward your {data?.max_open ?? 3} open.</span
      >
    </p>
  {:else}
    <p class="infobox"><Icon name="info" /><span>You withdrew this request{r.decided_at ? ` ${instantWords(r.decided_at, timeZone, now)}` : ''}. Nothing changed.</span></p>
  {/if}
  {#if r.note}
    <div class="field req-field">
      <span>Your note</span>
      <p class="infobox req-detail__note">“{r.note}”</p>
    </div>
  {/if}
  {#if r.state === 'waiting'}
    <!-- Phones keep the unfolded row short (board PhoneRequests): the key alone. -->
    <div class="req-detail__acts">{@render withdrawButton(r, phone)}{#if !phone}<span class="field__hint">Withdrawing frees one of your {data?.max_open ?? 3} open requests.</span>{/if}</div>
  {:else if changed}
    <div class="req-detail__acts"><a class="btn btn--primary" href={changed.href} onclick={(event) => follow(route, event, changed.href)}>{changed.label}</a></div>
  {:else if r.state === 'expired'}
    {@const again = againOf(r)}
    <div class="req-detail__acts">
      <a class="btn btn--primary" href={again.href} onclick={(event) => follow(route, event, again.href)}>Ask again for this week…</a>{#if again.hint}<span class="field__hint">{again.hint}</span>{/if}
    </div>
  {/if}
{/snippet}

{#snippet rowHead(r: MemberRequest)}
  {@const when = whenOf(r)}
  <span class="req-row__top"><b class="req-row__title">{titleOf(r)}</b>{@render chip(r)}</span>
  <span class="mono req-row__line">{when ? `${when} · ` : ''}{rowTail(r, timeZone, now)}</span>
{/snippet}

{#snippet empty()}
  {#if !data}
    {#if requests.error}
      <StateNote tone="error" icon="alert-circle" title="Your requests didn't load">
        {requests.error}
        {#snippet actions()}<button type="button" class="btn btn--primary" onclick={() => void requests.load()}>Try again</button>{/snippet}
      </StateNote>
    {:else}
      <LoadingState text="Loading your requests…" />
    {/if}
  {:else}
    <StateNote icon="message-square" title="No requests yet">
      Ask the admins to put you in a run, take you out, swap your place or change a weekly run.
      {#snippet actions()}<a class="btn btn--primary" href="/requests/new" onclick={(event) => follow(route, event, '/requests/new')}>Ask for a change…</a>{/snippet}
    </StateNote>
  {/if}
{/snippet}

<Modal bind:open={confirmOpen} eyebrow="My requests" title="Withdraw this request?" narrow className={phone ? 'own-sheet' : ''} returnFocus={() => withdrawKey ?? null}>
  {#if withdrawing}
    <p class="own-dialog__lead">{titleOf(withdrawing)} stops waiting on the admins and nothing changes. It still counts toward today's {data?.max_today ?? 6}.</p>
  {/if}
  {#snippet footer(close)}
    <button type="button" class="btn" onclick={close}>Keep it</button>
    <button type="button" class="btn btn--danger" onclick={() => void withdraw()}>Withdraw</button>
  {/snippet}
</Modal>

{#if phone}
  <h1 class="vh">My requests</h1>
  <section class="card window-fill req-window req-window--phone" aria-labelledby="{uid}-bar" data-fid="window">
    <div class="card__head" data-fid="window-bar">
      <h2 class="card__title" id="{uid}-bar">My requests</h2>
      <span class="req-window__end"
        ><a class="btn" href="/requests/new" onclick={(event) => follow(route, event, '/requests/new')}><Icon name="plus" />New<span class="vh"> request</span></a></span
      >
    </div>
    {@render notice?.()}
    {#if list.length}
      <div class="req-list req-list--phone" data-fid="requests-list">
        {#each list as r (r.id)}
          {@const on = r.id === openId}
          <div class="req-row" class:req-row--on={on} data-fid="requests-row">
            <a class="req-row__head" href="/requests?open={encodeURIComponent(r.id)}" aria-expanded={on} data-request={r.id} onclick={(event) => show(event, r.id)}>{@render rowHead(r)}</a>
            {#if on}
              <div class="req-row__more">{@render detail(r, false)}</div>
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <div class="req-empty">{@render empty()}</div>
    {/if}
  </section>
{:else}
  <div class="pageline" data-fid="page-line">
    <div class="pageline__head">
      <h1 class="pageline__title">My requests</h1>
      {#if data}<p class="pageline__context">· {waitingCount} waiting · {decidedCount} decided</p>{/if}
    </div>
    <div class="pageline__end"><a class="btn btn--primary" href="/requests/new" onclick={(event) => follow(route, event, '/requests/new')}>Ask for a change…</a></div>
  </div>
  {@render notice?.()}
  <section class="card window-fill req-window" aria-labelledby="{uid}-bar" data-fid="window">
    <div class="card__head" data-fid="window-bar">
      <h2 class="card__title" id="{uid}-bar">Requests</h2>
      <span class="req-window__end" data-fid="window-filters">{#if data}<span class="status-chip req-count">{counterWords(data)}</span>{/if}</span>
    </div>
    {#if list.length}
      <div class="req-window__body">
        <nav class="list-pane req-list" aria-label="Your requests" data-fid="requests-list">
          {#each list as r (r.id)}
            <a
              class="req-row req-row__head"
              class:req-row--on={r.id === openId}
              href="/requests?open={encodeURIComponent(r.id)}"
              aria-current={r.id === openId ? 'page' : undefined}
              data-request={r.id}
              data-fid="requests-row"
              onclick={(event) => show(event, r.id)}>{@render rowHead(r)}</a
            >
          {/each}
        </nav>
        <article class="req-detail" aria-labelledby="{uid}-detail" data-fid="requests-detail">
          {#if open}{@render detail(open, true)}{/if}
        </article>
      </div>
    {:else}
      <div class="req-empty">{@render empty()}</div>
    {/if}
  </section>
{/if}
