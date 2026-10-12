<!-- An admin's initial on their colour; a pencil badge while editing, faded and dashed once stale. Decorative: the words carry it. -->
<script lang="ts">
  import { Icon, initial } from '@kanade/ui';
  import type { Shown } from './presence';

  let { entry }: { entry: Pick<Shown, 'name' | 'mode' | 'stale' | 'tone'> } = $props();
</script>

<span class="presence__av presence__av--{entry.tone}" class:presence__av--stale={entry.stale} aria-hidden="true"
  >{initial(entry.name)}{#if entry.mode === 'editing'}<span class="presence__pen"><Icon name="edit" /></span>{/if}</span
>
