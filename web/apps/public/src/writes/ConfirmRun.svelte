<!--
  "Confirm it's you" for an answer or a move the server refused with `401
  reauth_required` (board ConfirmAnswer): nothing was saved; Sign in again
  comes back to the run with the answer picked (the Week, `?answer=`) or to
  its Move page with the slot picked (`?move_to=`), to press once more.
-->
<script lang="ts">
  import type { PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { runTitle } from '@kanade/ui';
  import { runWhen } from '../requests/form';
  import ConfirmFresh from './ConfirmFresh.svelte';
  import type { RunFlow } from './flow.svelte';
  import { choiceLabel, slotWords } from './runs';

  let { flow, session, current, phone }: { flow: RunFlow; session: PublicSession; current: PublicSessionRow | null; phone: boolean } = $props();

  const confirming = $derived(flow.confirming);
</script>

<ConfirmFresh
  bind:open={() => confirming !== null, (open) => !open && (flow.confirming = null)}
  what={confirming?.kind === 'move' ? 'Moving a run' : 'Changing your answer'}
  next={confirming?.next ?? '/'}
  {session}
  {current}
  {phone}
  hint={confirming?.kind === 'move' ? 'Asking the admins for a change needs the same sign-in.' : "Answering in Discord works as usual and doesn't need this."}
>
  {#if confirming?.kind === 'answer'}
    <b>Not saved yet:</b> {choiceLabel(confirming.answer)} for {runTitle(confirming.run)}, {runWhen(confirming.run, confirming.week)}. After signing in you're back on this run with
    {choiceLabel(confirming.answer)} picked; press it once more to save.
  {:else if confirming?.kind === 'move'}
    <b>Not saved yet:</b> {runTitle(confirming.run)} to {slotWords(confirming.week, confirming.slot)}. After signing in you're back on its Move page with that time picked;
    press Move it once more.
  {/if}
</ConfirmFresh>
