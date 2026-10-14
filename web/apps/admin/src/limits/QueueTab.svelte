<!--
  Queue (B_LimitsQueue): calls waiting for a permit, grouped by backend with
  the group's permits in its label row; groups with nothing waiting are named
  once underneath. Phones read each call as a two-line row.
-->
<script lang="ts">
  import type { BackendGroup } from '@kanade/api-types';

  let { groups, phone = false }: { groups: BackendGroup[]; phone?: boolean } = $props();

  const busy = $derived(groups.filter((g) => g.queue.length));
  const idle = $derived(groups.filter((g) => !g.queue.length).map((g) => g.name));
  const idleText = $derived(
    idle.length ? `Nothing is waiting for ${idle.length > 1 ? `${idle.slice(0, -1).join(', ')} or ${idle.at(-1)}` : idle[0]}.` : '',
  );
  const label = (g: BackendGroup) => `${g.queue.length} waiting · ${g.permits.in_use}/${g.permits.total} permits in use`;
</script>

{#if !busy.length}
  <p class="empty">Nothing is waiting.</p>
{:else if phone}
  {#each busy as g (g.name)}
    <section class="limits-plist" aria-labelledby="limits-q-{g.name}">
      <h3 class="limits-plist__head" id="limits-q-{g.name}" data-fid="limits-group-row">{g.name} <span class="limits-count mono">{label(g)}</span></h3>
      <ol class="limits-plist__rows">
        {#each g.queue as item (item.position)}
          <li class="limits-prow" data-fid="limits-row">
            <span class="limits-prow__line">
              <span class="limits-pos mono"><span class="vh">Position </span>{item.position}</span>
              <b class="limits-prow__lead">{item.who}</b>
              <b class="limits-prow__count mono">{item.waiting_s} s</b>
            </span>
            <span class="limits-prow__line limits-prow__sub mono">{item.kind}</span>
          </li>
        {/each}
      </ol>
    </section>
  {/each}
  {#if idleText}<p class="limits-note">{idleText}</p>{/if}
{:else}
  <table class="limits-table" data-fid="limits-list">
    <caption class="vh">Waiting for a permit, by backend and position</caption>
    <thead data-fid="limits-head">
      <tr>
        <th scope="col" class="limits-table__pos">Position</th>
        <th scope="col" class="limits-table__what">What</th>
        <th scope="col">For</th>
        <th scope="col" class="limits-table__num">Waiting</th>
      </tr>
    </thead>
    {#each busy as g (g.name)}
      <tbody>
        <tr class="limits-table__group" data-fid="limits-group-row">
          <th scope="colgroup" colspan="4">{g.name} <span class="limits-count mono">{label(g)}</span></th>
        </tr>
        {#each g.queue as item (item.position)}
          <tr class="limits-table__row" data-fid="limits-row">
            <td><span class="limits-pos mono">{item.position}</span></td>
            <td class="mono">{item.kind}</td>
            <th scope="row" class="limits-table__lead">{item.who}</th>
            <td class="mono limits-table__num"><b>{item.waiting_s} s</b></td>
          </tr>
        {/each}
      </tbody>
    {/each}
  </table>
  {#if idleText}<p class="limits-note">{idleText}</p>{/if}
{/if}
