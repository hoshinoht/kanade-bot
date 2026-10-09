<!--
  The member's answer on one of their runs, read-only for now: the boards'
  In / Maybe / Out group with the current answer pressed and every button
  disabled (answering from the portal arrives with member writes), and the
  way to answer today. Waiting presses none.
-->
<script lang="ts">
  import type { Answer } from '@kanade/api-types';

  let {
    answer,
    hint = true,
    ...rest
  }: {
    answer: Answer;
    /** The line under the group; My runs cards leave it to the window. */
    hint?: boolean;
    [key: `data-${string}`]: string | undefined;
  } = $props();

  const CHOICES: { value: Answer; label: string }[] = [
    { value: 'yes', label: 'In' },
    { value: 'maybe', label: 'Maybe' },
    { value: 'no', label: 'Out' },
  ];
  const now = $derived(CHOICES.find((c) => c.value === answer)?.label ?? 'not answered yet');
</script>

<div class="seg seg--tall member-answer" role="group" aria-label="Your answer: {now}" {...rest}>
  {#each CHOICES as choice (choice.value)}
    <button type="button" class="seg__btn" aria-pressed={answer === choice.value} aria-disabled="true">{choice.label}{answer === choice.value ? ' ✓' : ''}</button>
  {/each}
</div>
{#if hint}
  <p class="field__hint member-answer__hint">
    <span class="status-chip status-chip--warn">coming soon</span> Answering here; react on the run's card in Discord for now.
  </p>
{/if}
