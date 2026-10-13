<!--
  The tab's items as a keyboard-navigable listbox: boss and type lead, then
  who and when, then status badges in words.
-->
<script lang="ts">
  import type { Proposal } from '@kanade/api-types';
  import { Portrait, RowContent, StatusChip } from '@kanade/ui';
  import OptionList from '../shared/OptionList.svelte';
  import { FLAG_LABEL, FLAG_TONE, title, who } from './flags';

  let {
    items,
    selected,
    label,
    follow,
    onpick,
    fresh,
    updated = {},
    editing = '',
  }: {
    items: Proposal[];
    selected: string;
    label: string;
    follow: boolean;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
    /** Requests that changed under a refused edited approval: flagged "updated · version N". */
    updated?: Record<string, unknown>;
    /** The request being edited before approval (B_InboxEdit's "editing" chip). */
    editing?: string;
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
  fid="inbox-row"
  rowClass={(p) => ['inbox__option expandable-row', p.tab === 'extractor' && Boolean(p.bosses[0]) && 'inbox__option--art']}
  activeClass="inbox__option--active"
>
  {#snippet option(p)}
    {#if p.tab === 'extractor' && p.bosses[0]}
      <!-- Extractor rows (VarRail2): the boss art, then the lines; how much of the thread was used. -->
      <span class="inbox__art" aria-hidden="true"><Portrait boss={p.bosses[0]} size="md" /></span>
    {/if}
    <RowContent expanded={p.id === selected}>
      {#snippet compact()}<span class="inbox__what">{title(p)}</span> <span class="inbox__meta">· {who(p)} · {p.when}</span>{#if p.thread?.length}<span class="inbox__count"> · {p.thread.length} messages · {p.thread.filter((m) => m.used).length} used</span>{/if}{#each p.flags as flag (flag)} · {FLAG_LABEL[flag]}{/each}{#if p.is_question} · still a question{/if}{/snippet}
      <span class="inbox__lines">
      <span class="inbox__what">{title(p)}</span>
      <span class="inbox__meta"><span>{who(p)}</span> · <span class="mono">{p.when}</span></span>
      {#if p.tab === 'extractor' && p.thread?.length}
        <span class="inbox__count">{p.thread.length} message{p.thread.length === 1 ? '' : 's'} · {p.thread.filter((m) => m.used).length} used</span>
      {/if}
      {#if p.flags.length || p.is_question || p.id in updated || p.id === editing}
        <span class="inbox__badges">
          {#each p.flags as flag (flag)}<StatusChip tone={FLAG_TONE[flag] === 'danger' ? 'risk' : 'warn'} legacyTone={FLAG_TONE[flag]}>{FLAG_LABEL[flag]}</StatusChip>{/each}
          {#if p.is_question}<StatusChip tone="warn">still a question</StatusChip>{/if}
          {#if p.id === editing}<StatusChip tone="ok">editing</StatusChip>{:else if p.id in updated}<StatusChip tone="warn">updated · version {p.version}</StatusChip>{/if}
        </span>
      {/if}
      </span>
    </RowContent>
  {/snippet}
</OptionList>
