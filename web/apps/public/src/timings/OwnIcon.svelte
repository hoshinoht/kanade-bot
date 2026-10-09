<!--
  The three marks the ownership boards draw that the shared icon set lacks:
  the owner's crown (Lucide, ISC), Hand to…'s arrow and the "not saved yet"
  disk (Feather, MIT). Same box and stroke as `Icon`; decorative.
-->
<script lang="ts">
  let { name }: { name: 'crown' | 'arrow-right' | 'save' } = $props();
</script>

<svg
  class="icon"
  data-icon={name}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#if name === 'crown'}
    <path d="M11.6 3.3a.5.5 0 0 1 .8 0l3 5.6a1 1 0 0 0 1.5.3l4.3-3.7a.5.5 0 0 1 .8.5l-2.8 10.3a1 1 0 0 1-1 .7H5.8a1 1 0 0 1-1-.7L2 6a.5.5 0 0 1 .8-.5l4.3 3.7a1 1 0 0 0 1.5-.3zM5 21h14" />
  {:else if name === 'arrow-right'}
    <path d="M5 12h14M13 6l6 6-6 6" />
  {:else}
    <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2zM17 21v-8H7v8M7 3v5h8" />
  {/if}
</svg>
