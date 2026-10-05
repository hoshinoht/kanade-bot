<!--
  A member's or the admin's portrait (`/api/admin/members/{id}/avatar`,
  `/api/admin/me/avatar`): the server answers the cached Discord avatar or its
  monogram; the initial letter stays only when there is no URL or the image
  cannot load (offline, signed out). The host class sets size and shape.
-->
<script lang="ts">
  import { initial } from '@kanade/ui';

  let { src, name, class: className = '' }: { src: string | null; name: string; class?: string } = $props();
  let failed = $state<string | null>(null);
</script>

<span class="avatar {className}" aria-hidden="true">
  {#if src && failed !== src}
    <img class="avatar__img" {src} alt="" loading="lazy" decoding="async" onerror={() => (failed = src)} />
  {:else}{initial(name)}{/if}
</span>
