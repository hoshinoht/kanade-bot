<script lang="ts">
  import type { BlameEntry, Member } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import ActorName from '../history/ActorName.svelte';
  import { localAt, SURFACE_LABELS } from '../history/describe';
  import { memberLabel } from '../names/directory.svelte';
  import { send } from '../resource.svelte';
  import { blameField, blameValue } from './blame';

  let {
    runId,
    members,
    timezone,
    open = false,
    channel,
  }: {
    runId: string;
    members: Member[];
    timezone: string;
    /** Already open (the run pane's Changes tab). */
    open?: boolean;
    /** The run's home channel, so its blame row shows the name rather than the id when they match. */
    channel?: { id: string; name: string };
  } = $props();

  let entries = $state<BlameEntry[] | null>(null);
  let error = $state('');

  const names = (id: string) => memberLabel(members, id);
  const field = (f: string) => blameField(f, names);
  const value = (f: string, v: unknown) => (f === 'channel' && channel && v === channel.id ? channel.name : blameValue(f, v, timezone));

  async function load(event?: Event) {
    if ((event && !(event.currentTarget as HTMLDetailsElement).open) || entries) return;
    const result = await send((c) => c.get<BlameEntry[]>(`/api/admin/runs/${encodeURIComponent(runId)}/blame`));
    if (result.ok) entries = result.value;
    else error = result.message;
  }
  $effect(() => {
    if (open) void load();
  });
</script>

<!-- Blame: who last changed each part of this run, from the change history. -->
<details class="answers blame" {open} ontoggle={load}>
  <summary class="btn answers__summary"><Icon name="chevron-right" /> Who changed this</summary>
  <div class="answers__body blame__body">
    {#if error}<p class="field__error">{error}</p>
    {:else if !entries}<p class="note" aria-busy="true">Loading…</p>
    {:else if entries.length === 0}<p class="note">Nothing has changed since it was materialised.</p>
    {:else}
      <table class="blame__table">
        <caption class="vh">Last change per field</caption>
        <thead><tr><th scope="col">Field</th><th scope="col">Now</th><th scope="col">Changed by</th><th scope="col">When</th></tr></thead>
        <tbody>
          {#each entries as e (e.field)}
            <tr>
              <th scope="row">{field(e.field)}</th>
              <td class="mono blame__now">{value(e.field, e.value)}</td>
              <td class="blame__who"><ActorName actor={e.actor} {names} known={(id) => members.some((m) => m.id === id)} /> <span class="id">via {SURFACE_LABELS[e.surface] ?? e.surface}</span></td>
              <td class="mono blame__when"><a href="/history">#{e.seq}</a> · {localAt(e.at, timezone)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</details>

<style>
  /* The pane (about 320-480 px) and the phone sheet are too narrow for four
     columns: there each row stacks as the field, its value, then who and when
     on one wrapping line, so nothing runs past the pane's edge. */
  .blame__body {
    container-type: inline-size;
  }

  .blame__now {
    overflow-wrap: anywhere;
  }

  @container (max-width: 40rem) {
    .blame__table,
    .blame__table tbody {
      display: block;
    }

    /* Visually hidden, still the columns' names for assistive tech. */
    .blame__table thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }

    .blame__table tr {
      display: flex;
      flex-wrap: wrap;
      align-items: baseline;
      gap: 2px 0.75rem;
      padding: 0.5rem 0;
      border-bottom: 1px solid var(--line-soft);
    }

    .blame__table tbody tr:last-child {
      border-bottom: 0;
    }

    .blame__table th,
    .blame__table td {
      display: block;
      min-width: 0;
      padding: 0;
      border: 0;
    }

    .blame__table th,
    .blame__now {
      flex: 1 1 100%;
    }

    .blame__who,
    .blame__when {
      color: var(--dim-text);
      font-size: var(--fs-small);
      overflow-wrap: anywhere;
    }
  }
</style>
