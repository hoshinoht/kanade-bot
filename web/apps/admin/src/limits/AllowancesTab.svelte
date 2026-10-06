<!--
  Allowances (B_LimitsAllowances): members with chatbot access, their
  allowance as a bar of answers used against the count ("4 per 5 min"),
  and this window's use, with when its oldest answer frees up
  ("resets in", on the server's clock); a window in use can be reset from its row.
  Phones put allowance and window under the name, Reset at the end.
-->
<script lang="ts">
  import type { Allowance } from '@kanade/api-types';
  import { Icon, WavyProgress } from '@kanade/ui';
  import { resetSpan, windowWords } from './view';
  import { directory } from '../names/directory.svelte';
  import Name from '../names/Name.svelte';

  let {
    rows,
    phone = false,
    now = null,
    onreset,
  }: {
    rows: Allowance[];
    phone?: boolean;
    /** The server's clock (epoch ms) for the "resets in" countdowns. */
    now?: number | null;
    onreset: (id: string, name: string) => Promise<boolean>;
  } = $props();

  const nameOf = (a: Allowance) => directory.label('member', a.member.id, a.member.name);

  const rowId = (a: Allowance) => `limits-allowance-${a.member.id}`;

  // A held reset removes its button; focus moves to that member's name, unless
  // the user has already moved it. The success toast announces the reset.
  async function resetWindow(event: MouseEvent, a: Allowance) {
    const button = event.currentTarget as HTMLButtonElement;
    if (!(await onreset(a.member.id, nameOf(a)))) return;
    const active = document.activeElement;
    if (button.isConnected || (active && active !== document.body && active !== button)) return;
    document.getElementById(rowId(a))?.focus({ preventScroll: true });
  }
</script>

{#snippet who(a: Allowance)}
  <span class="limits-member">
    <b><Name kind="member" id={a.member.id} name={a.member.name} /></b>
    {#if a.staff}<span class="limits-chip">staff</span>{/if}
    {#if a.override}<span class="limits-chip">own allowance</span>{/if}
  </span>
{/snippet}

{#snippet quota(a: Allowance)}
  {#if a.allowance}
    {@const per = windowWords(a.allowance.per_s)}
    <span class="limits-allow">
      <!-- Flat: an allowance is not work in flight (Limits waves only permit bars). -->
      <WavyProgress
        value={a.used}
        max={a.allowance.count}
        wavy={false}
        label="{nameOf(a)}'s answers used"
        text="{a.used} of {a.allowance.count} answers used, {per} window"
      />
      <span class="limits-allow__per">{a.allowance.count} per {per}</span>
    </span>
  {:else}exempt{/if}
{/snippet}

{#snippet used(a: Allowance)}
  {#if !a.allowance}—{:else if a.used}<b>{a.used} used</b>, {a.allowance.count - a.used} left {@render resets(a)}{:else}<span class="limits-dim">idle</span>{/if}
{/snippet}

{#snippet resets(a: Allowance)}
  {@const span = resetSpan(a.resets_at, now)}
  <!-- Phones give it its own line under the use, without the separator. -->
  {#if span !== null}<span class="limits-dim limits-resets" class:limits-resets--line={phone}
      >{phone ? '' : '· '}{span ? `resets in ${span}` : 'resets now'}</span
    >{/if}
{/snippet}

{#snippet reset(a: Allowance)}
  {#if a.allowance && a.used}
    <button class="btn limits-reset" type="button" onclick={(event) => void resetWindow(event, a)} aria-label="Reset {nameOf(a)}'s window"
      ><Icon name="rotate-ccw" />Reset</button
    >
  {/if}
{/snippet}

{#if !rows.length}
  <p class="empty">No member has chatbot access.</p>
{:else if phone}
  <ul class="limits-plist__rows" aria-label="Chatbot allowances">
    {#each rows as a (a.member.id)}
      <li class="limits-prow limits-prow--action" data-fid="limits-row">
        <span class="limits-prow__body" id={rowId(a)} tabindex="-1">
          <span class="limits-prow__line">{@render who(a)}</span>
          <span class="limits-prow__line limits-prow__sub mono"><span class="limits-prow__quota">{@render quota(a)}</span><span>{@render used(a)}</span></span>
        </span>
        {@render reset(a)}
      </li>
    {/each}
  </ul>
{:else}
  <table class="limits-table" data-fid="limits-list">
    <caption class="vh">Chatbot allowances</caption>
    <thead data-fid="limits-head">
      <tr>
        <th scope="col">Member</th>
        <th scope="col" class="limits-table__allow">Allowance</th>
        <th scope="col" class="limits-table__window">This window</th>
        <th scope="col" class="limits-table__act"><span class="vh">Actions</span></th>
      </tr>
    </thead>
    <tbody>
      {#each rows as a (a.member.id)}
        <tr class="limits-table__row" data-fid="limits-row">
          <th scope="row" class="limits-table__lead" id={rowId(a)} tabindex="-1">{@render who(a)}</th>
          <td class="mono">{@render quota(a)}</td>
          <td class="mono">{@render used(a)}</td>
          <td class="limits-table__act">{@render reset(a)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .limits-resets {
    white-space: nowrap;
  }

  .limits-resets--line {
    display: block;
    text-align: right;
  }

  .limits-allow {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .limits-allow__per {
    white-space: nowrap;
  }

  /* Phones: one bar length on every row, so the bars compare. */
  .limits-prow__quota {
    flex: none;
    width: 104px;
  }
</style>
