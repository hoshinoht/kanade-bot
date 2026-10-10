<script lang="ts">
  import type { Tile } from './guide';

  let { tiles, label, fid }: { tiles: Tile[]; label: string; /** Fidelity tag (member portal). */ fid?: string } = $props();
</script>

<dl class="guide-tiles" aria-label={label} data-fid={fid}>
  {#each tiles as tile (tile.label)}
    <div class="guide-tile">
      <dt class="cap">{tile.label}</dt>
      <dd class="guide-tile__value mono">{tile.value}</dd>
      {#if tile.sub}<dd class="guide-tile__sub">{tile.sub}</dd>{/if}
    </div>
  {/each}
</dl>
