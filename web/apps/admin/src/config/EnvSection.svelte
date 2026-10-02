<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';

  import SettingsPanel from './SettingsPanel.svelte';

  let { env }: { env: ConfigView['env'] } = $props();
</script>

<SettingsPanel title="Set in the environment">
  {#snippet lead()}Read-only here: each needs an edit to the deployment's environment and a restart.{/snippet}
  <div class="env">
    <table class="settings__table" data-fid="cfg-table">
      <caption class="vh">Settings set in the environment</caption>
      <thead><tr><th scope="col">Setting</th><th scope="col">Value</th><th scope="col">Why it is env-only</th></tr></thead>
      <tbody>
        {#each env as row (row.key)}
          <tr>
            <th scope="row"><span class="env__name"><b>{row.label}</b><code class="capchip capchip--mono">{row.key}</code></span></th>
            <td class="mono">{row.value || 'not set'}</td>
            <td class="env__why">{row.reason}</td>
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
</style>
