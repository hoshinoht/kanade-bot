<!--
  The Party total's notes (board B_PartyTotalCrit): who a total counts at an
  assumed 100% critical rate, and who has no estimate (not in the total,
  never 0%). Nothing when there is neither.
-->
<script lang="ts">
  import type { TotalNotes } from './partyTotal';
  import { joinNames } from './shares';

  let { notes, fid }: { notes: TotalNotes; fid?: string } = $props();
</script>

{#if notes.assumed.length || notes.none.length}
  <p class="share__notes" data-fid={fid}>
    {#if notes.assumed.length}
      {@const rate = (actual: number | null) => (actual === null ? '' : ` (actual ${Math.round(actual * 10) / 10}%)`)}
      {@const one = notes.assumed.length === 1 ? notes.assumed[0] : null}
      <span class="share__note"
        ><span class="share__hatch" aria-hidden="true"></span>{one
          ? `* ${one.ign} at an assumed 100% critical rate${rate(one.actual)}`
          : `* ${joinNames(notes.assumed.map((a) => `${a.ign}${rate(a.actual)}`))} at an assumed 100% critical rate`}</span
      >
    {/if}
    {#if notes.none.length}
      <span class="share__note"
        ><span class="share__sw share__sw--none" aria-hidden="true"></span>{`${notes.none.length} with no estimate: ${joinNames(notes.none)} (not in the total, never 0%)`}</span
      >
    {/if}
  </p>
{/if}
