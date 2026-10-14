<script lang="ts">
  import BossWorkspace from './BossWorkspace.svelte';

  let { key, difficulty = '' }: { key: string; difficulty?: string } = $props();

</script>
<BossWorkspace selectedKey={key} {difficulty} />
