<!--
  The member's Move of their own run, this boss week only (boards
  Week-RunMine `week-move`, Move, PhoneMove): the shared MovePicker (day
  strip, time stepper, typed shortcut, suggestions), then what the portal
  checks about the pick — inside this boss week, no clash with the member's
  own runs (either blocks), teammates double-booked or not answered yet (a
  warning) — and the foot: "Moves to …", Cancel and Move. The pane and the
  phone page list only what fails or warns; the wide Move page lists every check.

  The pick survives a refused or stale move (the form is keyed per run, not
  per slot); a move made elsewhere while nothing was picked starts the
  picker afresh from the run's new slot. On a phone the foot stays at the
  bottom while the choices scroll (the page makes it the window's bar).
-->
<script lang="ts">
  import type { MemberMoveResult, MemberRun, MemberWeek } from '@kanade/api-types';
  import { dayLabel, Icon, liveRuns, MovePicker, namesIn, pickerRun, type MovePickerFids, type Slot } from '@kanade/ui';
  import { untrack } from 'svelte';
  import type { RunFlow } from '../writes/flow.svelte';
  import { moveChecks, slotWords } from '../writes/runs';

  let {
    run,
    week,
    memberId,
    flow,
    variant,
    initial,
    ends = '',
    step,
    legend,
    stepHint = '',
    note = '',
    fids = {},
    submitLabel = 'Move',
    onmoved,
    onrefused,
    oncancel,
    ...rest
  }: {
    run: MemberRun;
    week: MemberWeek;
    memberId: string;
    flow: RunFlow;
    /** pane: the run pane; page: the Move page; phone: the Move page on a phone. */
    variant: 'pane' | 'page' | 'phone';
    /** Where the pick starts (a Discord link's slot); the run's own slot when absent. */
    initial?: Slot;
    /** When the boss week ends ("Wed 14 Oct 23:59"), for the inside-the-week check. */
    ends?: string;
    step: number;
    legend: string;
    /** Replaces the picker's stepper hint ("↑ ↓ step 30 min · this week only"). */
    stepHint?: string;
    /** The line under the foot: what moves and who is told. */
    note?: string;
    fids?: MovePickerFids;
    submitLabel?: string;
    onmoved: (result: MemberMoveResult) => void;
    /** The move was not made (refused, stale, or waiting for a fresh sign-in); the pick stays. */
    onrefused?: () => void;
    /** Cancel: leave the page; without it Cancel puts the pick back. */
    oncancel?: () => void;
    [key: `data-${string}`]: string | undefined;
  } = $props();

  const own: Slot = $derived({ day: run.day, time: run.status === 'otot' ? null : run.time });
  const field = $derived(liveRuns(week.runs));
  const names = $derived(namesIn(week.runs));
  const subject = $derived(pickerRun(run));

  let epoch = $state(0);
  let value = $state<Slot>(untrack(() => initial ?? own));
  let blocked = $state(true);
  // The run's slot as the picker last started from it.
  let from = untrack(() => own);
  $effect.pre(() => {
    const now = own;
    untrack(() => {
      if (now.day === from.day && now.time === from.time) return;
      // Nothing picked: follow the run to its new slot. A pick stays put.
      if (value.day === from.day && value.time === from.time) epoch += 1;
      from = now;
    });
  });

  const checks = $derived(moveChecks(run, value, week, memberId));
  const busy = $derived(flow.busy === run.id);
  const canMove = $derived(!blocked && checks.inside && checks.yours.length === 0);
  // Only the Move page on a wide screen lists every check (board Move); the pane and the phone page (PhoneMove) list what fails or warns.
  const full = $derived(variant === 'page');
  const quiet = $derived(
    checks.quiet.length
      ? `${checks.quiet.join(', ')} ${checks.quiet.length === 1 ? "hasn't" : "haven't"} answered yet, so they may not see the new time before the run`
      : '',
  );

  async function submit(slot: Slot) {
    const check = moveChecks(run, slot, week, memberId);
    if (flow.busy || !check.inside || check.yours.length) return;
    const result = await flow.move(run, week, memberId, slot);
    if (result) onmoved(result);
    else onrefused?.();
  }

  function cancel() {
    if (oncancel) oncancel();
    else epoch += 1;
  }
</script>

{#snippet checklist()}
  {#if full || !checks.inside || checks.yours.length || checks.mates.length}
    <ul class="member-checks" aria-label="Checks" data-fid={full ? 'move-checks' : undefined}>
      {#if !checks.inside}
        <li class="member-checks__bad"><Icon name="x" />Outside this boss week or already past: pick a later time{ends ? ` before ${ends}` : ''}.</li>
      {:else if full}
        <li class="member-checks__ok"><Icon name="check" />Inside this boss week{ends ? ` (ends ${ends})` : ''}</li>
      {/if}
      {#if checks.yours.length}
        <li class="member-checks__bad"><Icon name="x" />Clashes with your {checks.yours.join(', ')}: pick another time.</li>
      {:else if full}
        <li class="member-checks__ok"><Icon name="check" />No clash with your other runs</li>
      {/if}
      {#each checks.mates as mate (mate)}
        <li class="member-checks__warn"><Icon name="alert-triangle" />Clash: {mate}. You can still move; they will be double-booked.</li>
      {/each}
      {#if quiet && full}
        <li class="member-checks__warn"><Icon name="alert-triangle" />{quiet}</li>
      {/if}
    </ul>
  {/if}
{/snippet}

<section class="member-move member-move--{variant}" aria-label="Move {run.bosses.map((b) => b.token).join(' + ')}" aria-busy={busy} {...rest}>
  <div class="member-move__body">
    {#key `${run.id}#${epoch}`}
      <MovePicker
        {fids}
        days={week.days}
        {subject}
        {own}
        initial={epoch === 0 ? initial : undefined}
        {field}
        {names}
        {step}
        variant={variant === 'phone' ? 'phone' : 'pane'}
        {legend}
        aside="{variant === 'phone' ? 'now' : 'from'} {dayLabel(week, own.day)} · {own.time ?? 'own time'}"
        hint={stepHint}
        {busy}
        showClash={false}
        bind:value
        bind:blocked
        onsubmit={(slot) => void submit(slot)}
        oncancel={oncancel ? cancel : undefined}
      />
    {/key}
    {@render checklist()}
  </div>
  <div class="movepick__foot member-move__foot">
    <p class="movepick__result" data-fid="move-result">
      <span class="cap">Moves to</span>
      <b class="mono movepick__to">{slotWords(week, value)}</b>
      {#if value.day !== own.day || value.time !== own.time}<s class="mono movepick__was"><span class="vh">was </span>{slotWords(week, own)}</s>{/if}
    </p>
    {#if note && variant === 'phone'}<p class="field__hint member-move__hint">{note}</p>{/if}
    <div class="movepick__acts" data-fid="move-submit">
      <button type="button" class="btn" disabled={busy} onclick={cancel}>Cancel</button>
      <button type="button" class="btn btn--primary btn--key movepick__go" disabled={busy || !canMove} onclick={() => void submit(value)}
        ><Icon name={variant === 'page' ? 'calendar' : 'move'} />{submitLabel}</button
      >
    </div>
  </div>
  {#if note && variant !== 'phone'}<p class="field__hint member-move__hint">{note}</p>{/if}
</section>
