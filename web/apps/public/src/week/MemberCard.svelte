<!--
  One run on the member's board or phone list (public-portal-plan § Week, D2):
  the admin card's body (`RunCardBody`, `AnswerBar`). The member's own runs
  get the full treatment and a YOU chip beside the clock; everyone else's are
  drawn faded without opacity: a dashed border on the row tint, no art or
  status stripe, outlined difficulty pills and "not in this run · view only".
  Both open the run (view only for the others).
-->
<script lang="ts">
  import type { MemberRun, MemberWeek } from '@kanade/api-types';
  import { AnswerBar, openPlaces, RunCardBody, runAccessibleName } from '@kanade/ui';
  import { partyLine } from '../member';

  let {
    run,
    week,
    memberId,
    selected = false,
    phone = false,
    onopen,
    ...rest
  }: {
    run: MemberRun;
    week: MemberWeek;
    memberId: string;
    selected?: boolean;
    phone?: boolean;
    onopen: (run: MemberRun) => void;
    /** `data-*` from the call site (the fidelity tag, `data-run`). */
    [key: `data-${string}`]: string | undefined;
  } = $props();

  const party = $derived(partyLine(run, memberId));
</script>

<button
  type="button"
  class="runcard runcard--{run.status} member-card"
  class:member-card--mine={run.mine}
  class:member-card--other={!run.mine}
  class:member-card--phone={phone}
  class:member-card--selected={selected}
  aria-label="{runAccessibleName(week, run)}, {run.mine ? 'you are in' : 'not in this run, view only'}"
  aria-current={selected ? 'true' : undefined}
  onclick={() => onopen(run)}
  {...rest}
>
  <RunCardBody {run} places={openPlaces(run)} art={run.mine}>
    {#snippet badge()}{#if run.mine}<span class="you-chip" aria-hidden="true">YOU</span>{/if}{/snippet}
  </RunCardBody>
  <span class="member-card__party" title={party}>{party}</span>
  {#if !run.mine}<span class="member-card__view" aria-hidden="true">not in this run · view only</span>{/if}
  <AnswerBar participants={run.participants} class="member-card__answers" />
</button>
