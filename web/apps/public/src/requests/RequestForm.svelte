<!--
  Ask for a change (`/requests/new`; boards RequestForm, RequestForm-Confirm,
  RequestForm-Limit, PhoneRequest): what the member wants (a connected
  group; a list of rows on phones), which run or weekly timing (only the
  ones that kind can take; a run a link named that it cannot stays, blocked,
  saying which kind can), the kind's own fields, an optional note, and a
  summary beside it with Send request. The title bar counts open and
  today's requests; at a limit the key is off with the reason. A send that
  needs a fresh sign-in opens "Confirm it's you" and comes back to this
  form with the draft in the address (`draftReturn`). Prefilled from `?run=`,
  `?kind=` and Discord's `?fixed=&change=edit|remove&day=&time=`.
-->
<script lang="ts">
  import type { PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { newIdempotencyKey } from '@kanade/client';
  import { DayStrip, DIFFICULTY_WORDS, fromMinutes, Icon, LoadingState, MultiSelect, Portrait, Select, TimeStepper, toMinutes, type StripDay, type Toaster } from '@kanade/ui';
  import '@kanade/ui/styles/select.scss';
  import { untrack, type Snippet } from 'svelte';
  import type { Route } from '../route.svelte';
  import { WEEKDAYS } from '../timings/ownership';
  import type { MemberTimingsList } from '../timings/timings.svelte';
  import type { MemberWeeks } from '../weeks.svelte';
  import ConfirmFresh from '../writes/ConfirmFresh.svelte';
  import { follow } from '../writes/follow';
  import {
    asideWords,
    bodyOf,
    cleanNote,
    counterWords,
    draftProposal,
    draftReturn,
    headline,
    keptSubject,
    KINDS,
    limitOf,
    limitToast,
    needsWeeks,
    NOTE_MAX,
    prefill,
    proposalWords,
    subjectChoices,
    type Draft,
    type Kind,
  } from './form';
  import type { MemberRequestsList } from './requests.svelte';
  import './requests.scss';

  let {
    requests,
    weeks,
    timings,
    route,
    session,
    current,
    toaster,
    phone,
    notice,
  }: {
    requests: MemberRequestsList;
    weeks: MemberWeeks;
    /** The member's weekly timings: the subjects of a weekly change, and a timing a Discord link names. */
    timings: MemberTimingsList;
    route: Route;
    session: PublicSession;
    /** This device's row in the session list, for when it signed in (Confirm it's you). */
    current: PublicSessionRow | null;
    toaster: Toaster;
    phone: boolean;
    notice?: Snippet;
  } = $props();

  const uid = $props.id();
  const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;
  const memberId = $derived(session.member.id);
  const data = $derived(requests.data);
  const options = $derived(data?.options ?? null);

  // Fresh counts and choices every time the form opens; the timings name weekly subjects.
  $effect(() =>
    untrack(() => {
      void requests.load();
      if (!timings.data) void timings.load();
    }),
  );

  // The address is read once: `?run=` without `?kind=` waits for the weeks (Join someone else's run, else Leave).
  let draft = $state<Draft | null>(null);
  $effect(() => {
    if (draft) return;
    const params = route.params;
    if (needsWeeks(params) && !weeks.this && !weeks.offline && !weeks.error) return;
    draft = untrack(() => prefill(params, (id) => weeks.find(id)?.run.mine ?? false));
  });

  const timingList = $derived(timings.data?.timings ?? []);
  const choices = $derived(draft ? subjectChoices(draft.kind, { this: weeks.this, next: weeks.next }, timingList, draft.subject) : []);
  const picked = $derived(draft ? choices.find((c) => c.value === draft!.subject && !c.blocked) : undefined);
  const timing = $derived(draft?.subject.startsWith('fixed:') ? timingList.find((t) => `fixed:${t.id}` === draft!.subject) : undefined);
  const result = $derived(draft ? bodyOf(draft, memberId) : null);
  const limit = $derived(data ? limitOf(data) : null);
  const missing = $derived(result && 'missing' in result ? result.missing : '');
  const noteLength = $derived(draft ? [...cleanNote(draft.note)].length : 0);

  const weekly = $derived(draft?.kind === 'new_fixed' || draft?.kind === 'change_fixed');
  const bossWords = $derived(
    draft?.kind === 'new_fixed'
      ? draft.bosses.split(/[\s,+]+/).filter(Boolean).join(' + ') || 'a weekly run'
      : (picked?.title ?? (draft?.kind === 'change_fixed' ? 'a weekly run' : 'a run')),
  );
  const title = $derived(draft ? headline(draft.kind, bossWords) : '');
  const swapWith = $derived(draft?.kind === 'swap' && draft.with ? (options?.members.find((m) => m.id === draft!.with)?.name ?? '') : '');
  // The summary's line: the run and its channel, or what the weekly timing becomes.
  const line = $derived.by(() => {
    if (!draft) return '';
    if (weekly) {
      const proposed = draftProposal(draft, options);
      const changes = proposed && (proposed.day !== null || proposed.time !== null || proposed.channel !== null || proposed.party !== null);
      if (changes) return proposalWords({ proposed }, draft.kind === 'change_fixed' ? (timing ?? null) : null);
      return picked?.long ?? '';
    }
    const where = picked ? `${picked.long}${picked.channel ? ` · ${picked.channel}` : ''}` : '';
    return swapWith ? `${where}${where ? ' · ' : ''}${swapWith} takes your place` : where;
  });

  // The weekly day strip runs in boss-week order from the reset day (as the admin's Fixed editor).
  const resetDay = $derived(Math.max(0, WEEKDAYS.findIndex((d) => d === weeks.this?.days[0]?.dow)));
  const weekdayStrip: StripDay[] = $derived(WEEKDAYS.map((_, i) => (resetDay + i) % 7).map((value) => ({ value, dow: WEEKDAYS[value]!, label: WEEKDAYS[value]! })));
  const shownDay = $derived(draft?.day ?? (draft?.kind === 'change_fixed' ? (timing?.weekday ?? null) : null));
  const shownTime = $derived(draft?.time || (draft?.kind === 'change_fixed' ? (timing?.time ?? '') : ''));
  const memberOptions = $derived((options?.members ?? []).map((m) => ({ value: m.id, label: m.name })));
  const channelOptions = $derived((options?.channels ?? []).map((c) => ({ value: c.id, label: c.name })));
  // A weekly change shows the timing's party until the member picks another (empty keeps it).
  const shownParty = $derived(draft?.party.length || draft?.kind !== 'change_fixed' ? (draft?.party ?? []) : (timing?.party.map((m) => m.id).filter((id) => id !== memberId) ?? []));

  function pickKind(kind: Kind) {
    if (!draft || draft.kind === kind) return;
    const subject = keptSubject(subjectChoices(kind, { this: weeks.this, next: weeks.next }, timingList, draft.subject), draft.subject);
    draft.kind = kind;
    draft.subject = subject;
  }

  // "Confirm it's you": the draft rides in the address through Discord and back.
  let confirming = $state(false);
  const back = $derived(draft ? draftReturn(draft) : { path: '/requests/new', noteKept: true });
  const hasNote = $derived(draft ? cleanNote(draft.note) !== '' : false);

  let sendKey = $state<HTMLButtonElement>();

  async function send() {
    if (!draft || limit || requests.busy || !result || !('body' in result)) return;
    const sent = title;
    // One key per press: the client's own retries repeat it; a new press is a new request.
    const outcome = await requests.send(result.body, newIdempotencyKey());
    if (!outcome) return;
    switch (outcome.kind) {
      case 'done':
        toaster.show({ message: `Sent: ${sent}. The admins review it; Discord tells you when one decides.`, tone: 'ok' });
        route.go('/requests', { open: outcome.request.id });
        return;
      case 'reauth':
        confirming = true;
        return;
      case 'limit':
        toaster.show({ message: limitToast(outcome.limit, requests.data), tone: 'error', action: { label: 'My requests', run: () => route.go('/requests') } });
        return;
      case 'refused':
        toaster.show({ message: `Couldn't send the request: ${outcome.words}`, tone: 'error' });
    }
  }
</script>

{#snippet counter()}
  {#if data}<span class="status-chip req-count" class:status-chip--risk={limit !== null}>{phone ? `${data.open} of ${data.max_open} open` : counterWords(data)}</span>{/if}
{/snippet}

{#snippet kinds(d: Draft)}
  <fieldset class="field req-field">
    <legend class="label">What do you want?</legend>
    <div class="req-seg" class:req-seg--stack={phone} role="radiogroup" aria-label="Request type" data-fid="request-kind">
      {#each KINDS as k (k.id)}
        <label class="req-seg__opt" class:req-seg__opt--on={d.kind === k.id}>
          <input class="req-pick" type="radio" name="{uid}-kind" value={k.id} checked={d.kind === k.id} onchange={() => pickKind(k.id)} />
          <span class="req-seg__label">{k.label}</span>
          {#if phone && d.kind === k.id}<Icon name="check" />{/if}
        </label>
      {/each}
    </div>
  </fieldset>
{/snippet}

{#snippet subject(d: Draft)}
  {#if d.kind !== 'new_fixed'}
    {@const label = d.kind === 'change_fixed' ? 'Which weekly run' : 'Which run'}
    {@const scope = d.kind === 'join' ? "runs you're not in" : d.kind === 'change_fixed' ? "weekly runs you're in" : "only runs you're in"}
    {#if phone}
      <div class="field req-field" data-fid="request-run">
        <span>{label}</span>
        <Select
          {label}
          value={d.subject}
          placeholder={choices.length ? 'Pick one' : 'Nothing to pick this week or next'}
          disabled={!choices.length}
          noun="runs"
          options={choices.map((c) => ({ value: c.value, label: `${c.title} · ${c.when}`, sub: c.blocked || (c.next ? 'next week' : undefined), disabled: Boolean(c.blocked) }))}
          onchange={(value) => (d.subject = value)}
        />
      </div>
    {:else}
      <fieldset class="field req-field" data-fid="request-run">
        <legend class="label">{label} · {scope}</legend>
        {#if choices.length}
          <div class="req-runs" role="radiogroup" aria-label={label}>
            {#each choices as c (c.value)}
              {@const on = c.value === d.subject && !c.blocked}
              <label class="req-run" class:req-run--on={on} class:req-run--blocked={c.blocked}>
                <input class="req-pick" type="radio" name="{uid}-subject" value={c.value} checked={on} disabled={Boolean(c.blocked)} onchange={() => (d.subject = c.value)} />
                {#if c.bosses[0]}<Portrait boss={c.bosses[0]} />{/if}
                <b class="req-run__title">{c.title}</b>
                {#if !c.blocked && c.bosses[0]}<span class="pill pill--{c.bosses[0].difficulty}">{DIFFICULTY_WORDS[c.bosses[0].difficulty].toUpperCase()}</span>{/if}
                {#if c.next}<span class="status-chip">next week</span>{/if}
                {#if c.blocked}<span class="req-run__blocked">{c.blocked}</span>{/if}
                <span class="mono req-run__when">{c.when}</span>
                {#if on}<Icon name="check" />{/if}
              </label>
            {/each}
          </div>
        {:else}
          <p class="field__hint">
            {#if !weeks.this || (d.kind === 'change_fixed' && !timings.data)}Loading…{:else}Nothing to pick: no {scope} this week or next.{/if}
          </p>
        {/if}
      </fieldset>
    {/if}
  {/if}
{/snippet}

{#snippet fields(d: Draft)}
  {#if d.kind === 'swap'}
    <div class="field req-field">
      <span>Who takes your place</span>
      <Select label="Who takes your place" value={d.with} placeholder="Pick a member" noun="members" options={memberOptions} onchange={(value) => (d.with = value)} />
    </div>
  {:else if d.kind === 'new_fixed'}
    <label class="field req-field">
      <span>Bosses</span>
      <input class="mono" bind:value={d.bosses} placeholder="HLimbo, HStar" autocomplete="off" />
    </label>
  {/if}
  {#if weekly}
    <div class="req-fields">
      <div class="field req-field">
        <span>Day{d.kind === 'change_fixed' ? ' · keep or change' : ''}</span>
        <DayStrip days={weekdayStrip} value={shownDay} label="Day" compact onpick={(value) => (d.day = value)} />
      </div>
      <div class="field req-field">
        <label class="label" for="{uid}-time">Time</label>
        <TimeStepper
          id="{uid}-time"
          value={TIME.test(shownTime) ? toMinutes(shownTime) : null}
          step={30}
          start={21 * 60}
          draft={d.time || shownTime}
          placeholder="21:00"
          invalid={Boolean(d.time) && !TIME.test(d.time)}
          ontype={(text) => (d.time = text)}
          onchange={(minutes) => (d.time = fromMinutes(minutes))}
        />
      </div>
      <div class="field req-field">
        <span>Party channel</span>
        <Select
          label="Party channel"
          value={d.channel}
          placeholder={d.kind === 'change_fixed' ? 'Keep the channel' : 'Pick a channel'}
          noun="channels"
          options={channelOptions}
          onchange={(value) => (d.channel = value)}
        />
      </div>
      <div class="field req-field">
        <span>Party{d.kind === 'new_fixed' ? ' · you are in it' : ' · keep or change'}</span>
        <MultiSelect label="Party" size="field" noun="members" values={shownParty} options={memberOptions} onchange={(values) => (d.party = values)} />
      </div>
    </div>
  {/if}
{/snippet}

{#snippet note(d: Draft)}
  <label class="field req-field" data-fid="request-note">
    <span>Note for the admins · optional</span>
    <textarea class="req-note" bind:value={d.note} rows="3" aria-describedby="{uid}-note-hint"></textarea>
    <span class="field__hint req-note__hint" class:req-note__hint--over={noteLength > NOTE_MAX} id="{uid}-note-hint"
      >One line, up to {NOTE_MAX} characters · <span class="mono">{noteLength}/{NOTE_MAX}</span></span
    >
  </label>
{/snippet}

{#snippet key(full: boolean)}
  <button
    bind:this={sendKey}
    type="button"
    class="btn btn--primary btn--key req-send"
    class:btn--full={full}
    aria-disabled={limit !== null || Boolean(missing) || requests.busy}
    aria-describedby={limit ? `${uid}-limit` : missing ? `${uid}-missing` : undefined}
    onclick={() => void send()}>{requests.busy ? 'Sending…' : 'Send request'}</button
  >
{/snippet}

{#snippet limitNote()}
  {#if limit}
    <p class="flash flash--warn req-limit" id="{uid}-limit" data-fid="request-limit">
      <Icon name="alert-triangle" /><span><b>{limit.lead}</b> {limit.text}</span>
    </p>
  {/if}
{/snippet}

{#snippet confirm()}
  <ConfirmFresh bind:open={confirming} what="Sending a request" next={back.path} {session} {current} {phone} returnFocus={() => sendKey ?? null}>
    <b>Your request is kept:</b>
    {title}{line ? `, ${line}` : ''}{hasNote && back.noteKept ? ', with your note' : ''}. You come back to this page and press Send once more.{hasNote && !back.noteKept
      ? ' Your note is too long to carry through the sign-in; copy it first.'
      : ''}
  </ConfirmFresh>
{/snippet}

{#if phone}
  <h1 class="vh">Ask for a change</h1>
  <section class="card window-fill req-window req-window--phone" aria-labelledby="{uid}-bar" data-fid="request-form">
    <div class="card__head" data-fid="window-bar">
      <h2 class="card__title" id="{uid}-bar">New request</h2>
      <span class="req-window__end">{@render counter()}</span>
    </div>
    {@render notice?.()}
    {#if draft}
      <div class="req-form__body">
        {@render kinds(draft)}
        {@render subject(draft)}
        {@render fields(draft)}
        {@render note(draft)}
        {@render limitNote()}
        <p class="infobox"><Icon name="info" /><span>{asideWords(draft.kind)}</span></p>
        {#if missing && !limit}<p class="field__hint" id="{uid}-missing">{missing}</p>{/if}
      </div>
      <div class="req-form__foot">
        <a class="btn btn--key" href="/requests" onclick={(event) => follow(route, event, '/requests')}>Cancel</a>
        {@render key(false)}
      </div>
    {:else}
      <LoadingState text="Loading your runs…" />
    {/if}
  </section>
{:else}
  <div class="pageline" data-fid="page-line">
    <div class="pageline__head">
      <h1 class="pageline__title">Ask for a change</h1>
      <p class="pageline__context">· admins review every request</p>
    </div>
  </div>
  {@render notice?.()}
  <section class="card window-fill req-window" aria-labelledby="{uid}-bar" data-fid="request-form">
    <div class="card__head" data-fid="window-bar">
      <h2 class="card__title" id="{uid}-bar">New request</h2>
      <span class="req-window__end" data-fid="window-filters">{@render counter()}</span>
    </div>
    {#if draft}
      <div class="req-window__body">
        <div class="req-form__body">
          {@render kinds(draft)}
          {@render subject(draft)}
          {@render fields(draft)}
          {@render note(draft)}
        </div>
        <aside class="side-pane req-aside" aria-labelledby="{uid}-summary" data-fid="request-aside">
          <p class="cap">Summary</p>
          <h2 class="req-aside__title" id="{uid}-summary">{title}</h2>
          {#if line}<p class="req-aside__line mono">{line}</p>{/if}
          <p class="req-aside__text">{asideWords(draft.kind)}</p>
          <span class="req-aside__spacer"></span>
          {@render limitNote()}
          {@render key(true)}
          {#if limit}
            <a class="btn btn--full" href="/requests" onclick={(event) => follow(route, event, '/requests')}>Open My requests</a>
          {:else if missing}
            <p class="field__hint" id="{uid}-missing">{missing}</p>
          {:else if data}
            <p class="field__hint">You can withdraw it while it waits. Up to {data.max_open} open and {data.max_today} a day.</p>
          {/if}
        </aside>
      </div>
    {:else}
      <LoadingState text="Loading your runs…" />
    {/if}
  </section>
{/if}
<!-- A dialog of the page, not of the window (board RequestForm-Confirm). -->
{#if draft}{@render confirm()}{/if}
