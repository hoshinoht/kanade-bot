<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';
  import { Icon, type Toaster } from '@kanade/ui';
  import { copyText } from '../shared/copy';

  import SettingsPanel from './SettingsPanel.svelte';

  let { env, toaster }: { env: ConfigView['env']; toaster: Toaster } = $props();

  // The raw env form (ids, `thu`, the URL), not the human value shown.
  async function copy(key: string, value: string) {
    if (await copyText(value)) toaster.show({ message: `Copied ${key}.`, tone: 'ok' });
    else toaster.show({ message: `Couldn't copy here; ${key} is ${value}`, tone: 'error' });
  }
</script>

<SettingsPanel title="Set in the environment">
  {#snippet lead()}Read-only here: each needs an edit to the deployment's environment and a restart.{/snippet}
  <div class="env">
    <table class="settings__table" data-fid="cfg-table">
      <caption class="vh">Settings set in the environment</caption>
      <thead
        ><tr
          ><th scope="col">Setting</th><th scope="col">Value</th><th scope="col">Why it is env-only</th><th scope="col" class="env__act"><span class="vh">Copy</span></th></tr
        ></thead
      >
      <tbody>
        {#each env as row (row.key)}
          <tr>
            <th scope="row"><span class="env__name"><b>{row.label}</b><code class="capchip capchip--mono">{row.key}</code></span></th>
            <td class="mono">{row.value || 'not set'}</td>
            <td class="env__why">{row.reason}</td>
            <td class="env__act">
              {#if row.copy != null}
                {@const value = row.copy}
                <button class="btn btn--ghost env__copy" type="button" aria-label="Copy {row.key}" title="Copy {row.key}" onclick={() => void copy(row.key, value)}
                  ><Icon name="copy" /></button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</SettingsPanel>

<style>
  .env {
    overflow-x: auto;
  }

  .env tbody :is(td, th) {
    height: auto;
    padding-block: 0.5rem;
  }

  .env__name {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
  }

  .env__why {
    color: var(--dim-text);
  }

  .env__act {
    width: 2.75rem;
  }

  .env tbody td.env__act {
    padding: 0 0.25rem;
  }

  .env__copy.btn {
    justify-content: center;
    width: 32px;
    min-width: 32px;
    height: 32px;
    padding: 0;
    border: 0;
    border-radius: 10px;
    color: var(--ink);
  }

  .env__copy.btn:hover {
    background: var(--row-hover);
  }
</style>
