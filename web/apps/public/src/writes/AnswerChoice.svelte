<!--
  The member's answer on one of their runs (boards Week-RunMine, PhoneRun,
  MyRuns, ConfirmAnswer): In / Maybe / Out, 44 px tall, the current answer
  pressed with a tick. A press shows at once and rolls back when refused
  (`RunFlow`); while it is out the group is busy and other presses wait.
  Back from "Confirm it's you", the choice the member made (`picked`) is
  marked and says it is not saved yet: one more press saves it. A run that
  no longer takes answers (done, cancelled) keeps the group, every button off.
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { yours } from '../member';
  import type { WeekKey } from '../weeks.svelte';
  import type { RunFlow } from './flow.svelte';
  import { answerable, CHOICES, choiceLabel, type Choice } from './runs';

  let {
    run,
    week,
    which,
    memberId,
    flow,
    picked = null,
    onpress,
    hint = true,
    label = '',
    ...rest
  }: {
    run: MemberRun;
    week: MemberWeek;
    which: WeekKey;
    memberId: string;
    flow: RunFlow;
    /** The choice a fresh sign-in came back with, not saved yet. */
    picked?: Choice | null;
    /** After any press (the page forgets `picked`). */
    onpress?: () => void;
    /** The line under the group; My runs cards leave it out. */
    hint?: boolean;
    /** The group's name; "Your answer" with the current one by default. */
    label?: string;
    [key: `data-${string}`]: string | undefined;
  } = $props();

  const answer = $derived(yours(run, memberId)?.answer ?? 'waiting');
  const open = $derived(answerable(run, memberId));
  const busy = $derived(flow.busy === run.id);

  function press(choice: Choice) {
    if (!open || flow.busy) return;
    onpress?.();
    // Pressing the answer already saved changes nothing, unless it is the one waiting to be saved.
    if (choice === answer && choice !== picked) return;
    void flow.answer(run, week, which, memberId, choice);
  }
</script>

<div class="seg seg--tall member-answer" role="group" aria-label={label || `Your answer: ${choiceLabel(answer)}`} aria-busy={busy} {...rest}>
  {#each CHOICES as choice (choice.value)}
    <button
      type="button"
      class="seg__btn"
      class:member-answer__btn--picked={picked === choice.value}
      aria-pressed={answer === choice.value}
      aria-disabled={!open || (busy && answer !== choice.value) ? 'true' : undefined}
      onclick={() => press(choice.value)}>{choice.label}{answer === choice.value ? ' ✓' : ''}</button
    >
  {/each}
</div>
{#if picked && open}
  <p class="field__hint member-answer__hint member-answer__hint--picked" role="status">
    <b>Not saved yet:</b> {choiceLabel(picked)}. Press it once more to save.
  </p>
{:else if hint}
  <p class="field__hint member-answer__hint">
    {open ? 'Same as reacting on the card in Discord. Changes at once.' : `This run is ${run.status === 'cancelled' ? 'cancelled' : 'done'}; answers are closed.`}
  </p>
{/if}
