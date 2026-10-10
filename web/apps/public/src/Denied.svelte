<!--
  `/?login_error=not_eligible` (boards Denied, PhoneDenied): neutral on
  purpose. It never says whether the account is in the guild or which role
  is missing, and no session exists (the server wrote no row and no cookie),
  so the boards' "Signed in as …" line has nothing to name and is left out.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import GateCard from './GateCard.svelte';
  import GateWindow from './GateWindow.svelte';
  import PhoneNote from './PhoneNote.svelte';
  import { discordStart } from './landing';

  let { identity, zone, phone, next, onswitch }: { identity: Identity | null; zone: string; phone: boolean; next: string; onswitch: () => void } = $props();
  const TITLE = "This account can't see the schedule";
  const TEXT = 'The schedule is for guild members with the bossing role. If you should have access, ask an admin, then try again.';
</script>

{#if phone}
  <GateCard bar="Sign in">
    <PhoneNote icon="lock" title={TITLE} hint="To switch, change account in Discord first.">
      {TEXT}
      {#snippet actions()}
        <button type="button" class="btn btn--primary btn--key btn--full" onclick={onswitch}>Use another account</button>
        <a class="btn btn--key btn--full" href={discordStart(next)}>Try again</a>
      {/snippet}
    </PhoneNote>
  </GateCard>
{:else}
  <GateWindow {identity} bar="Not eligible" {zone} short>
    <h1 class="gate__lead" id="gate-lead">{TITLE}</h1>
    <p>{TEXT}</p>
    <div class="gate__actions" data-fid="state-actions">
      <button type="button" class="btn btn--key" onclick={onswitch}>Use another account</button>
      <a class="btn btn--primary btn--key" href={discordStart(next)}>Try again</a>
    </div>
  </GateWindow>
{/if}
