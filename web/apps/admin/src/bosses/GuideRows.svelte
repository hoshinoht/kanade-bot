<!-- Titled guide rows: a bold title over one line of text; a plain string is text only. -->
<script lang="ts">
  import type { GuideItem } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { itemParts } from './guide';

  let { items, mark, label }: { items: GuideItem[]; mark?: 'risk' | 'ok'; label?: string } = $props();
</script>

<ul class="guide-rows" aria-label={label}>
  {#each items as item, index (index)}
    {@const part = itemParts(item)}
    <li class="guide-row">
      {#if mark}<span class="guide-row__mark guide-row__mark--{mark}"><Icon name={mark === 'risk' ? 'alert-triangle' : 'check'} /></span>{/if}
      <span class="guide-row__words">{#if part.title}<strong>{part.title}</strong>{/if}<span>{part.text}</span></span>
    </li>
  {/each}
</ul>
