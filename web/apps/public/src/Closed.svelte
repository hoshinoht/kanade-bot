<!--
  The portal is closed (boards Closed, PhoneClosed): the admins' switch is
  off, so sign-in is hidden. Reminders and answers in Discord still work;
  "Check again" re-reads the status.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import GateCard from './GateCard.svelte';
  import GateWindow from './GateWindow.svelte';
  import PhoneNote from './PhoneNote.svelte';

  let { identity, zone, phone, oncheck }: { identity: Identity | null; zone: string; phone: boolean; oncheck: () => void } = $props();
  const TITLE = "The schedule isn't open right now";
  const TEXT = "The guild's admins have closed the portal. Reminders and answers still work in Discord.";
</script>

{#if phone}
  <GateCard bar="Closed">
    <PhoneNote icon="moon" title={TITLE}>
      {TEXT}
      {#snippet actions()}<button type="button" class="btn btn--primary btn--key btn--full" onclick={oncheck}>Check again</button>{/snippet}
    </PhoneNote>
  </GateCard>
{:else}
  <GateWindow {identity} bar="Closed" {zone} short muted>
    <span><span class="status-chip">closed</span></span>
    <h1 class="gate__lead" id="gate-lead">{TITLE}</h1>
    <p>{TEXT}</p>
    <button type="button" class="btn btn--key gate__key" onclick={oncheck} data-fid="gate-key"><Icon name="refresh-cw" />Check again</button>
  </GateWindow>
{/if}
