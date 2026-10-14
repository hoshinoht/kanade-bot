<!--
  Experiment A: a short-wait indicator that morphs circle → pill → rounded
  square while turning (styles in _experiments.scss). Reduced motion shows a
  still rounded square.
-->
<script lang="ts">
  let { label = 'Loading', size = 'md' }: { label?: string; size?: 'sm' | 'md' } = $props();
</script>

<span class="xp-loading xp-loading--{size}" role="status" aria-label={label}><span class="xp-loading__shape"></span></span>
