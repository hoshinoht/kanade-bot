<!--
  The tab's drafts as a keyboard listbox (B_Drafts `drafts-list`), as the
  Inbox list: the boss portrait of the first staged run, the title, how many
  changes and on which History change, then Up to date / Base moved (or how
  it closed) and when it was last edited.
-->
<script lang="ts">
  import type { Boss, DraftRow } from '@kanade/api-types';
  import { Portrait, StatusChip } from '@kanade/ui';
  import OptionList from '../shared/OptionList.svelte';
  import { closedLabel, editedAt } from './drafts';

  let {
    items,
    selected,
    label,
    follow,
    portraits,
    timeZone,
    now = '',
    onpick,
    fresh,
  }: {
    items: DraftRow[];
    selected: string;
    label: string;
    follow: boolean;
    /** The first staged run's boss per draft, once read (absent: none yet). */
    portraits: Record<string, Boss | null>;
    timeZone: string;
    /** The server's clock (`Week.generated_at`): today's edits show the time only. */
    now?: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
  } = $props();
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();

  export const focusOn = async (id: string) => list?.focusOn(id);
</script>

<OptionList
  bind:this={list}
  {label}
  {items}
  {selected}
  {follow}
  {onpick}
  {fresh}
  fid="drafts-row"
  rowClass={() => 'inbox__option inbox__option--art drafts__row'}
  activeClass="inbox__option--active"
>
  {#snippet option(d)}
    {@const boss = portraits[d.id]}
    <span class="inbox__art drafts__art" aria-hidden="true">{#if boss}<Portrait {boss} size="md" />{/if}</span>
    <span class="inbox__lines">
      <span class="inbox__what">{d.title}</span>
      <span class="inbox__meta"><b class="mono drafts__count">{d.staged}</b> staged · on <span class="mono">#{d.base}</span></span>
      <span class="inbox__badges drafts__state">
        {#if d.status !== 'open'}
          <StatusChip tone={d.status === 'merged' ? 'ok' : 'neutral'}>{closedLabel(d)}</StatusChip>
        {:else if d.up_to_date}
          <StatusChip tone="ok">Up to date</StatusChip>
        {:else}
          <StatusChip tone="warn">Base moved</StatusChip>
        {/if}
        <span class="mono drafts__edited">edited {editedAt(d.updated_at, timeZone, now)}</span>
      </span>
    </span>
  {/snippet}
</OptionList>
