<script lang="ts">
  import type { ConfigView } from '@kanade/api-types';

  let { env }: { env: ConfigView['env'] } = $props();
</script>

<h3 class="settings__title">Set in the environment</h3>
<div class="table-wrap">
  <table>
    <caption>Read-only here: each needs an edit to the deployment's environment and a restart.</caption>
    <thead><tr><th scope="col">Setting</th><th scope="col">Value</th><th scope="col">Why it is env-only</th></tr></thead>
    <tbody>
      {#each env as row (row.key)}
        <tr>
          <th scope="row">{row.label}<br /><code class="chip chip--mono">{row.key}</code></th>
          <td class="mono">{row.value || 'not set'}</td>
          <td>{row.reason}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
