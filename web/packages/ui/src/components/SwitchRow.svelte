<!--
  One setting kept in this browser (Account › This browser, both apps): its
  title and what it does, then a switch that applies at once. With `icon`
  the row leads with it, as the member portal's Motion row.
-->
<script lang="ts">
  import Icon, { type IconName } from './Icon.svelte';

  let { id, title, note, on, icon, onflip }: { id: string; title: string; note: string; on: boolean; icon?: IconName; onflip: () => void } = $props();
</script>

<div class="account-row" class:account-row--plain={!icon}>
  {#if icon}<span class="account-lead"><Icon name={icon} /></span>{/if}
  <span class="account-row__text">
    <span class="account-row__title" id="{id}-t">{title}</span>
    <span class="account-row__sub" id="{id}-d">{note}</span>
  </span>
  <button type="button" role="switch" class="switch" aria-checked={on} aria-labelledby="{id}-t" aria-describedby="{id}-d" onclick={onflip}
    ><span class="switch__knob" aria-hidden="true"></span></button
  >
</div>
