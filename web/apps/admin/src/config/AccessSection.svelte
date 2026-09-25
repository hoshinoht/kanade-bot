<!-- v4 partials/access.html: the bot role's permissions per channel. -->
<script lang="ts">
  import type { AccessReport } from '@kanade/api-types';
  import { Icon, type Toaster } from '@kanade/ui';
  import { Resource, send } from '../resource.svelte';

  let { toaster }: { toaster: Toaster } = $props();

  const report = new Resource<AccessReport>('/api/admin/access');
  $effect(() => {
    void report.load();
  });
  let busy = $state(false);

  const COLUMNS = [
    ['view', 'See'],
    ['send', 'Post'],
    ['history', 'Read history'],
    ['embed', 'Embeds'],
    ['react', 'React'],
    ['manage_messages', 'Manage messages'],
  ] as const;
  const blocked = $derived(report.data?.rows.filter((r) => !r.view || !r.send || !r.history || !r.embed || !r.react) ?? []);
  const noManage = $derived(report.data?.rows.filter((r) => !r.manage_messages) ?? []);

  async function recheck() {
    if (busy) return;
    busy = true;
    const result = await send((c) => c.post<AccessReport>('/api/admin/access/recheck', {}));
    busy = false;
    if (result.ok) report.data = result.value;
    toaster.show({ message: result.ok ? `Checked again at ${result.value.checked_at}.` : `Couldn't check: ${result.message}`, tone: result.ok ? 'ok' : 'error' });
  }
</script>

<h3 class="settings__title">Channel access</h3>
<div class="settings__actions">
  <button class="btn" type="button" aria-disabled={busy} onclick={() => void recheck()}>Check again</button>
</div>
<p class="field__error" role="alert">{report.error}</p>
{#if report.data}
  {@const data = report.data}
  {#if !data.connected}
    <p class="note">The bot isn't connected to the guild right now, so its permissions can't be checked. Try again once it has logged in.</p>
  {:else}
    <p class="note" role="status">
      Checked {data.checked_at}.
      {#if blocked.length}<strong>{blocked.length} channel{blocked.length === 1 ? '' : 's'}</strong> will not get reminders.
      {:else}Every channel gets its reminders.{/if}
      {#if noManage.length}{noManage.length} cannot have old reactions tidied.{/if}
    </p>
    <div class="table-wrap">
      <table>
        <caption class="vh">The bot's permissions in each channel</caption>
        <thead>
          <tr><th scope="col">Channel</th>{#each COLUMNS as [, label] (label)}<th scope="col">{label}</th>{/each}</tr>
        </thead>
        <tbody>
          {#each data.rows as row (row.id)}
            <tr>
              <th scope="row">
                {row.name}
                {#if row.digest}<span class="chip">digest</span>{/if}
                {#if !row.watched && !row.digest}<span class="chip chip--waiting">not watched</span>{/if}
              </th>
              {#each COLUMNS as [key, label] (key)}
                <td><span class="status {row[key] ? 'status--confirmed' : 'status--at_risk'}"><Icon name={row[key] ? 'check' : 'x'} label="{label}: {row[key] ? 'granted' : 'missing'}" /></span></td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="note">
      A <Icon name="x" label="cross" /> means the bot's role is missing that permission in that channel — the reminders for its runs will not go
      out. Fix it in <strong>Edit Channel → Permissions</strong>. <strong>Manage messages</strong> is the one exception: without it the reminders
      still go out, but the bot cannot take somebody's old reaction off, so a person who switches ❌ to ✅ in Discord is counted as both.
    </p>
  {/if}
{/if}
