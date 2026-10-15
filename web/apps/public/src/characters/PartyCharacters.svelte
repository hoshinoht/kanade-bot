<!--
  The run pane's Party · characters (board M_RunParty), between Your answer
  and Move, on a run the member is on: the label, the planning-only note and
  the attribution once at the top (the attribution only while the member's
  own row carries NEXON values), then each party member, "You" outlined and
  first, with their answer and character line. Between the note and the
  members: the run total and Who carries (`PartyShare`). Module off or
  another run: the route answers 404 and nothing is drawn.
-->
<script lang="ts">
  import type { MemberRun, RunPartyCharacters } from '@kanade/api-types';
  import { ANSWER_MARKS } from '@kanade/ui';
  import PartyCharLine from './PartyCharLine.svelte';
  import PartyNote from './PartyNote.svelte';
  import PartyShare from './PartyShare.svelte';

  let { party, run }: { party: RunPartyCharacters; run: MemberRun } = $props();

  const slots = $derived([...party.slots].sort((a, b) => Number(b.you) - Number(a.you)));
  const answers = $derived(new Map(run.participants.map((p) => [p.id, p.answer])));
</script>

<section class="member-pane__sec" aria-labelledby="party-chars-h" data-fid="week-party-chars">
  <p class="cap" id="party-chars-h">Party · characters</p>
  <PartyNote {party} />
  <PartyShare {party} {run} />
  <ul class="party-chars" aria-label="Party characters">
    {#each slots as slot, i (slot.member.id)}
      {@const answer = answers.get(slot.member.id) ?? 'waiting'}
      <li class="party-char" class:party-char--me={slot.you} data-fid={i === 0 ? 'party-char-row' : undefined}>
        <span class="party-char__top"
          ><span class="party-char__name">{slot.you ? 'You' : slot.member.name}</span><span class="chip chip--{answer}"
            ><span aria-hidden="true">{ANSWER_MARKS[answer].mark}</span> {ANSWER_MARKS[answer].word}</span
          ></span
        >
        <PartyCharLine {slot} tagged={i === 0} />
      </li>
    {/each}
  </ul>
</section>
