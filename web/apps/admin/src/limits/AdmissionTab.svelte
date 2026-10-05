<!--
  Admission (B_LimitsAdmission): what the Kanata gateway refused in its
  window, grouped by scope — backend groups, then the gateway key — each row
  with its kind, scope chip, where, count and last time. Phones
  (B_PhoneLimitsAdmission) read each refusal as a two-line row.
-->
<script lang="ts">
  import type { Refusal } from '@kanade/api-types';
  import { REFUSAL, refusalGroups, serverTime } from './view';

  let { refusals, zone, phone = false }: { refusals: Refusal[]; zone: string | undefined; phone?: boolean } = $props();

  const groups = $derived(refusalGroups(refusals));
  const kind = (r: Refusal) => REFUSAL[r.kind] ?? r.kind;
</script>

{#snippet scope(r: Refusal)}
  <span class="limits-scope mono" class:limits-scope--key={r.scope === 'key'}>{r.scope === 'key' ? 'key-level' : 'backend'}</span>
{/snippet}

{#if !groups.length}
  <p class="empty">Nothing was refused.</p>
{:else if phone}
  {#each groups as group (group.scope)}
    <section class="limits-plist" aria-labelledby="limits-a-{group.scope}">
      <h3 class="limits-plist__head cap" id="limits-a-{group.scope}" data-fid="limits-group-row">{group.label} <span class="limits-count mono">{group.total} refused</span></h3>
      <ul class="limits-plist__rows">
        {#each group.rows as r (r.kind + r.target)}
          <li class="limits-prow" data-fid="limits-row">
            <span class="limits-prow__line">
              <b class="limits-prow__lead">{kind(r)}</b>{@render scope(r)}
              <b class="limits-prow__count mono" aria-label="{r.count} refused">{r.count}</b>
            </span>
            <span class="limits-prow__line limits-prow__sub mono"><span>{r.target}</span><span>last {serverTime(r.last_at, zone)}</span></span>
          </li>
        {/each}
      </ul>
    </section>
  {/each}
{:else}
  <table class="limits-table" data-fid="limits-list">
    <caption class="vh">Admission refusals by kind</caption>
    <thead data-fid="limits-head">
      <tr>
        <th scope="col">Kind</th>
        <th scope="col" class="limits-table__scope">Scope</th>
        <th scope="col" class="limits-table__where">Where</th>
        <th scope="col" class="limits-table__num limits-table__count">Count</th>
        <th scope="col" class="limits-table__last">Last</th>
      </tr>
    </thead>
    {#each groups as group (group.scope)}
      <tbody>
        <tr class="limits-table__group" data-fid="limits-group-row">
          <th scope="colgroup" colspan="5">{group.label} <span class="limits-count mono">{group.total} refused</span></th>
        </tr>
        {#each group.rows as r (r.kind + r.target)}
          <tr class="limits-table__row" data-fid="limits-row">
            <th scope="row" class="limits-table__lead">{kind(r)}</th>
            <td>{@render scope(r)}</td>
            <td class="mono">{r.target}</td>
            <td class="mono limits-table__num limits-table__big">{r.count}</td>
            <td class="mono">{serverTime(r.last_at, zone)}</td>
          </tr>
        {/each}
      </tbody>
    {/each}
  </table>
{/if}
