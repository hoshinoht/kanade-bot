<script lang="ts">
  import type { RunStatus } from '@kanade/api-types';
  import Icon from './Icon.svelte';
  import { STATUS_ICONS, STATUS_WORDS } from '../format';
  import { RUN_TONE } from '../tone';

  let { status, words = false, pill = false }: { status: RunStatus; words?: boolean; /** The semantic pill profile. */ pill?: boolean } = $props();
</script>

<!-- Icon plus the word: visibly on sheets and tables, for screen readers on cards. -->
<span class={pill ? `tone tone--${RUN_TONE[status]}` : `status status--${status}`} title={STATUS_WORDS[status]}>
  <Icon name={STATUS_ICONS[status]} />
  {#if words}{STATUS_WORDS[status]}{:else}<span class="vh">{STATUS_WORDS[status]}</span>{/if}
</span>
