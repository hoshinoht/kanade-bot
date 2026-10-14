<!-- v4 partials/access.html: the bot role's permissions per channel. -->
<script lang="ts">
  import type { AccessReport } from '@kanade/api-types';
  import { Icon, type Toaster } from '@kanade/ui';
  import { Resource, send } from '../resource.svelte';
  import Name from '../names/Name.svelte';
  import SettingsPanel from './SettingsPanel.svelte';

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
  const problem = (r: AccessReport['rows'][number]) => COLUMNS.some(([key]) => !r[key]);
  let only = $state<'all' | 'problems'>('all');
  const rows = $derived((report.data?.rows ?? []).filter((r) => only === 'all' || problem(r)));

  async function recheck() {
    if (busy) return;
    busy = true;
    const result = await send((c) => c.post<AccessReport>('/api/admin/access/recheck', {}));
    busy = false;
    if (result.ok) report.data = result.value;
    toaster.show({ message: result.ok ? `Checked again at ${result.value.checked_at}.` : `Couldn't check: ${result.message}`, tone: result.ok ? 'ok' : 'error' });
  }
</script>

<SettingsPanel title="Channel access">
  {#snippet lead()}What the bot may do in each channel.{/snippet}
  {#if report.error}<p class="field__error" role="alert">{report.error}</p>{/if}
  {#if report.data}
    {@const data = report.data}
    {#if !data.connected}
      <p class="settings__box">The bot isn't connected to the guild right now, so its permissions can't be checked. Try again once it has logged in.</p>
      <div class="access__tools"><button class="btn" type="button" aria-disabled={busy} onclick={() => void recheck()}>Check again</button></div>
    {:else}
      {#if noManage.length || blocked.length}
        <p class="settings__box settings__box--risk">
          <span>
            A <Icon name="x" label="cross" /> means the bot's role is missing that permission in that channel — the reminders for its runs will not go
            out. Fix it in <strong>Edit Channel → Permissions</strong>. <strong>Manage messages</strong> is the one exception: without it the reminders
            still go out, but the bot cannot take somebody's old reaction off, so a person who switches ❌ to ✅ in Discord is counted as both.
          </span>
        </p>
      {/if}
      <div class="access__tools">
        <p class="settings__cardnote" role="status">
          Checked {data.checked_at} ·
          {#if blocked.length}<strong>{blocked.length} channel{blocked.length === 1 ? '' : 's'}</strong> will not get reminders.
          {:else}every channel gets its reminders.{/if}
          {#if noManage.length}{noManage.length} cannot have old reactions tidied.{/if}
        </p>
        <div class="seg" role="group" aria-label="Show">
          <button type="button" aria-pressed={only === 'all'} onclick={() => (only = 'all')}>All <span class="mono">{data.rows.length}</span></button>
          <button type="button" aria-pressed={only === 'problems'} onclick={() => (only = 'problems')}
            >Problems <span class="mono">{data.rows.filter(problem).length}</span></button
          >
        </div>
        <button class="btn" type="button" aria-disabled={busy} onclick={() => void recheck()}>Check again</button>
      </div>
      <div class="access">
        <table class="settings__table" data-fid="cfg-table">
          <caption class="vh">The bot's permissions in each channel</caption>
          <thead>
            <tr><th scope="col">Channel</th>{#each COLUMNS as [, label] (label)}<th scope="col">{label}</th>{/each}</tr>
          </thead>
          <tbody>
            {#each rows as row (row.id)}
              <tr class:access__bad={problem(row)}>
                <th scope="row">
                  <Name kind="channel" id={row.id} name={row.name} />
                  {#if row.digest}<span class="capchip">digest</span>{/if}
                  {#if !row.watched && !row.digest}<span class="capchip">not watched</span>{/if}
                </th>
                {#each COLUMNS as [key, label] (key)}
                  <td
                    ><span class="access__mark" class:access__mark--no={!row[key]}
                      ><Icon name={row[key] ? 'check' : 'x'} label="{label}: {row[key] ? 'granted' : 'missing'}" />{#if !row[key]}<span aria-hidden="true">missing</span>{/if}</span
                    ></td
                  >
                {/each}
              </tr>
            {:else}
              <tr><td colspan={COLUMNS.length + 1} class="note">No channel has a problem.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</SettingsPanel>

<style>
  .access__tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.625rem;
  }

  .access__tools > p {
    flex: 1 1 16rem;
  }

  .access {
    overflow-x: auto;
  }

  .access__bad :is(td, th) {
    background: color-mix(in srgb, var(--risk) 7%, var(--row));
  }

  .access td {
    text-align: center;
  }

  .access__mark {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--ok-text);
    font-weight: 700;
  }

  .access__mark--no {
    color: color-mix(in srgb, var(--risk-text) 80%, var(--ink));
  }
</style>
