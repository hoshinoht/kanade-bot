<script lang="ts" module>
  /** Two letters from a catalog key: `MaleficStar` -> `MS`, `Carling` -> `Ca`. */
  export function monogram(key: string): string {
    const capitals = key.replace(/[^A-Z]/g, '');
    return capitals.length >= 2 ? capitals.slice(0, 2) : key.slice(0, 2);
  }
</script>

<script lang="ts">
  import type { Attachment } from 'svelte/attachments';
  import type { Boss } from '@kanade/api-types';

  let { boss, size = 'sm' }: { boss: Boss; size?: 'sm' | 'md' } = $props();

  const src = $derived(size === 'sm' ? boss.portrait_sm : boss.portrait);

  // The boss's hue goes in through CSSOM: a style attribute is blocked by style-src 'self'.
  const hue: Attachment<HTMLElement> = (node) => {
    node.style.setProperty('--mono-hue', String(boss.hue));
  };
</script>

<!-- A portrait, or a monogram in the same box so nothing shifts (v4 macros.portrait). -->
{#if src}
  <img class="portrait portrait--{size}" {src} alt="" loading="lazy" decoding="async" />
{:else}
  <span class="portrait portrait--{size} portrait--mono" aria-hidden="true" {@attach hue}>{monogram(boss.key)}</span>
{/if}
