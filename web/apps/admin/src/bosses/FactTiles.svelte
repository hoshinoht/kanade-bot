<script lang="ts">
  import type { Tile } from './guide';

  let { tiles, label }: { tiles: Tile[]; label: string } = $props();
</script>

<dl class="guide-tiles" aria-label={label}>
  {#each tiles as tile (tile.label)}
    <div class="guide-tile">
      <dt class="cap">{tile.label}</dt>
      {#if tile.rows?.length}
        <dd class="guide-tile__rows">
          <ul aria-label="{tile.label} by party size">
            {#each tile.rows as row (row.label)}<li><span>{row.label}</span><span class="mono">{row.value}</span></li>{/each}
          </ul>
        </dd>
      {:else}
        <dd class="guide-tile__value mono">{tile.value}</dd>
      {/if}
      {#if tile.sub}<dd class="guide-tile__sub">{tile.sub}</dd>{/if}
    </div>
  {/each}
</dl>
