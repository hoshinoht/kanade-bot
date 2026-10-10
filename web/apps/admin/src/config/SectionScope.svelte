<!-- Scopes a section's save bars, so the contents list can mark the section that holds unsaved changes. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { provideDirty } from './dirty';

  let { section, report, children }: { section: string; report: (id: string, dirty: boolean) => void; children: Snippet } = $props();

  provideDirty({ set: (id, dirty) => report(`${section}/${id}`, dirty) });
</script>

{@render children()}
