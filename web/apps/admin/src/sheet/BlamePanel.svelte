<script lang="ts">
  import type { BlameEntry, Member } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { actorName, localAt, SURFACE_LABELS } from '../history/describe';
  import { send } from '../resource.svelte';

  let { runId, members, timezone }: { runId: string; members: Member[]; timezone: string } = $props();

  let entries = $state<BlameEntry[] | null>(null);
  let error = $state('');

  const names = (id: string) => {
    const name = members.find((m) => m.id === id)?.name ?? `member ${id}`;
    return members.filter((m) => m.name === name).length > 1 ? `${name} #${id}` : name;
  };
  const FIELD: Record<string, string> = { day: 'Day', time: 'Time', status: 'Status', participants: 'Roster' };
  const field = (f: string) => FIELD[f] ?? (f.startsWith('answer:') ? `${names(f.slice(7))}'s answer` : f);
  const value = (v: unknown) => (Array.isArray(v) ? `${v.length} people` : v === null ? 'cleared' : String(v));

  async function load(event: Event) {
    if (!(event.currentTarget as HTMLDetailsElement).open || entries) return;
    const result = await send((c) => c.get<BlameEntry[]>(`/api/admin/runs/${encodeURIComponent(runId)}/blame`));
    if (result.ok) entries = result.value;
    else error = result.message;
  }
</script>

<!-- Blame: who last changed each part of this run, from the change history. -->
<details class="answers blame" ontoggle={load}>
  <summary class="btn answers__summary"><Icon name="chevron-right" /> Who changed this</summary>
  <div class="answers__body">
    {#if error}<p class="field__error">{error}</p>
    {:else if !entries}<p class="note" aria-busy="true">Loading…</p>
    {:else if entries.length === 0}<p class="note">Nothing has changed since it was materialised.</p>
    {:else}
      <table>
        <caption class="vh">Last change per field</caption>
        <thead><tr><th scope="col">Field</th><th scope="col">Now</th><th scope="col">Changed by</th><th scope="col">When</th></tr></thead>
        <tbody>
          {#each entries as e (e.field)}
            <tr>
              <th scope="row">{field(e.field)}</th>
              <td class="mono">{value(e.value)}</td>
              <td>{actorName(e.actor, names)} <span class="id">via {SURFACE_LABELS[e.surface] ?? e.surface}</span></td>
              <td class="mono"><a href="/history">#{e.seq}</a> · {localAt(e.at, timezone)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</details>
