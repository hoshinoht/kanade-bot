<!--
  Feather icons (MIT, (c) 2013-2023 Cole Bemis), the same paths v4's
  partials/icons.html draws. Decorative by default; pass `label` when the
  mark is the only thing that says it.
-->
<script lang="ts" module>
  export type IconName =
    | 'check'
    | 'x'
    | 'alert-triangle'
    | 'alert-circle'
    | 'clock'
    | 'chevron-right'
    | 'refresh-cw'
    | 'search'
    | 'move'
    | 'rotate-ccw'
    | 'wifi-off'
    | 'external-link'
    | 'log-out';
</script>

<script lang="ts">
  let { name, label = '' }: { name: IconName; label?: string } = $props();
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
  {#if name === 'check'}
    <polyline points="20 6 9 17 4 12" />
  {:else if name === 'x'}
    <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
  {:else if name === 'alert-triangle'}
    <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
    <line x1="12" y1="9" x2="12" y2="13" /><line x1="12" y1="17" x2="12.01" y2="17" />
  {:else if name === 'alert-circle'}
    <circle cx="12" cy="12" r="10" />
    <line x1="12" y1="8" x2="12" y2="12" /><line x1="12" y1="16" x2="12.01" y2="16" />
  {:else if name === 'clock'}
    <circle cx="12" cy="12" r="10" /><polyline points="12 6 12 12 16 14" />
  {:else if name === 'chevron-right'}
    <polyline points="9 18 15 12 9 6" />
  {:else if name === 'refresh-cw'}
    <polyline points="23 4 23 10 17 10" /><polyline points="1 20 1 14 7 14" />
    <path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15" />
  {:else if name === 'search'}
    <circle cx="11" cy="11" r="8" /><line x1="21" y1="21" x2="16.65" y2="16.65" />
  {:else if name === 'move'}
    <polyline points="5 9 2 12 5 15" /><polyline points="9 5 12 2 15 5" /><polyline points="15 19 12 22 9 19" />
    <polyline points="19 9 22 12 19 15" /><line x1="2" y1="12" x2="22" y2="12" /><line x1="12" y1="2" x2="12" y2="22" />
  {:else if name === 'rotate-ccw'}
    <polyline points="1 4 1 10 7 10" /><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10" />
  {:else if name === 'external-link'}
    <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
    <polyline points="15 3 21 3 21 9" /><line x1="10" y1="14" x2="21" y2="3" />
  {:else if name === 'log-out'}
    <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" /><polyline points="16 17 21 12 16 7" />
    <line x1="21" y1="12" x2="9" y2="12" />
  {:else if name === 'wifi-off'}
    <line x1="1" y1="1" x2="23" y2="23" /><path d="M16.72 11.06A10.94 10.94 0 0 1 19 12.55" />
    <path d="M5 12.55a10.94 10.94 0 0 1 5.17-2.39" /><path d="M10.71 5.05A16 16 0 0 1 22.58 9" />
    <path d="M1.42 9a15.91 15.91 0 0 1 4.7-2.88" /><path d="M8.53 16.11a6 6 0 0 1 6.95 0" />
    <line x1="12" y1="20" x2="12.01" y2="20" />
  {/if}
</svg>
{#if label}<span class="vh">{label}</span>{/if}
