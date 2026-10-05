<script lang="ts">
  import type { KnowledgeSource } from '@kanade/api-types';
  import { StatusChip } from '@kanade/ui';
  import { sourceCounts } from './guide';

  let { sources }: { sources: KnowledgeSource[] } = $props();
  const meta = (source: KnowledgeSource) =>
    [`by ${source.author}`, source.kind, `fetched ${source.fetched}`, ...(source.updated ? [`updated ${source.updated}`] : [])].join(' · ');
</script>

<ul class="guide-source-kinds" aria-label="Sources by kind">
  {#each sourceCounts(sources) as row (row.kind)}<li><StatusChip>{row.kind} <b class="mono">{row.count}</b></StatusChip></li>{/each}
</ul>
<ul class="guide-sources">
  {#each sources as source (source.url)}
    <li>
      <a href={source.url} rel="noopener noreferrer" target="_blank">{source.title}</a>
      <span class="guide-sources__meta">{meta(source)}</span>
    </li>
  {/each}
</ul>
<p class="note">Our own paraphrase of these sources; the authors are credited above.</p>
