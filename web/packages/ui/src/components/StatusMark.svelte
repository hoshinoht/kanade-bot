<script lang="ts">
  import type { RunStatus } from '@kanade/api-types';
  import Icon from './Icon.svelte';
  import { STATUS_ICONS, STATUS_WORDS } from '../format';

  let { status, words = false }: { status: RunStatus; words?: boolean } = $props();
</script>

<!-- Icon plus the word: visibly on sheets and tables, for screen readers on cards. -->
<span class="status status--{status}" title={STATUS_WORDS[status]}>
  <Icon name={STATUS_ICONS[status]} />
  {#if words}{STATUS_WORDS[status]}{:else}<span class="vh">{STATUS_WORDS[status]}</span>{/if}
</span>
