<!--
  The Ownership tab's open requests as a listbox, oldest first: the timing
  leads, then who asks and who owns it now, then when it expires.
-->
<script lang="ts">
  import type { OwnershipRequest } from '@kanade/api-types';
  import { Portrait } from '@kanade/ui';
  import OptionList from '../shared/OptionList.svelte';
  import { expiresIn, handover, ownershipTitle } from './ownership';

  let {
    items,
    selected,
    follow,
    now,
    onpick,
    fresh,
  }: {
    items: OwnershipRequest[];
    selected: string;
    follow: boolean;
    /** The server's clock (`Week.generated_at`); empty leaves the expiry out. */
    now: string;
    onpick: (id: string, open: boolean) => void;
    fresh?: ReadonlySet<string>;
  } = $props();
  let list = $state<{ focusOn: (id: string) => Promise<void> }>();

  export const focusOn = async (id: string) => list?.focusOn(id);
</script>

<OptionList
  bind:this={list}
  label="Ownership requests"
  {items}
  {selected}
  {follow}
  {onpick}
  {fresh}
  fid="ownership-row"
  rowClass={(r) => ['inbox__option', Boolean(r.bosses[0]) && 'inbox__option--art']}
  activeClass="inbox__option--active"
>
  {#snippet option(r)}
    {#if r.bosses[0]}<span class="inbox__art" aria-hidden="true"><Portrait boss={r.bosses[0]} size="md" /></span>{/if}
    <span class="inbox__lines">
      <span class="inbox__what">{ownershipTitle(r)}</span>
      <span class="inbox__meta"><span class="mono">{r.weekday_name} {r.time}</span> · <span>{handover(r)}</span></span>
      {#if expiresIn(r, now)}<span class="inbox__count">{expiresIn(r, now)}</span>{/if}
    </span>
  {/snippet}
</OptionList>
