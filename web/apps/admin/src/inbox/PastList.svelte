<!--
  The Past tab's closed items as a listbox, newest first: the outcome and
  summary lead, then the kind and source, then who closed it and when.
-->
<script lang="ts">
  import type { PastItem } from '@kanade/api-types';
  import { StateNote } from '@kanade/ui';
  import { localAt } from '../history/describe';
  import OptionList from '../shared/OptionList.svelte';
  import { SOURCE_LABEL } from './flags';
  import OutcomeChip from './OutcomeChip.svelte';

  let {
    items,
    selected,
    follow,
    timeZone,
    onpick,
  }: {
    items: PastItem[];
    selected: string;
    follow: boolean;
    timeZone: string;
    onpick: (id: string, open: boolean) => void;
  } = $props();
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();

  export const focusOn = async (id: string) => list?.focusOn(id);
</script>

{#if items.length}
  <OptionList
    bind:this={list}
    label="Past items"
    {items}
    {selected}
    {follow}
    {onpick}
    fid="past-row"
    rowClass={() => 'inbox__option past__option'}
    activeClass="inbox__option--active"
  >
    {#snippet option(p)}
      <span class="inbox__lines">
        <span class="past__rowhead"><OutcomeChip outcome={p.outcome} /><span class="inbox__what past__summary">{p.summary}</span></span>
        <span class="inbox__meta">{p.kind_label} · {SOURCE_LABEL[p.source]}</span>
        <span class="inbox__meta">{p.decided_by ? `${p.decided_by.name} · ` : ''}<time class="mono" datetime={p.decided_at}>{localAt(p.decided_at, timeZone)}</time></span>
      </span>
    {/snippet}
  </OptionList>
{:else}
  <div class="state-pane">
    <StateNote icon="clock" title="Nothing closed yet">No closed items yet. Approved, rejected and expired changes are kept here.</StateNote>
  </div>
{/if}
