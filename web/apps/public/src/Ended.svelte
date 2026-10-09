<!--
  A session that ended while in use (`401 unauthenticated` mid-session;
  boards Ended, PhoneEnded): it timed out, was signed out from another
  device, or the member is no longer eligible. Nothing of the account stays
  on screen. The time-outs are server settings the portal is not told, so
  the words name them without numbers.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import GateCard from './GateCard.svelte';
  import GateWindow from './GateWindow.svelte';
  import PhoneNote from './PhoneNote.svelte';
  import { discordStart } from './landing';

  let { identity, zone, phone, next, at }: { identity: Identity | null; zone: string; phone: boolean; next: string; /** Epoch ms. */ at: number } = $props();
  const TITLE = "You've been signed out";
  const time = $derived(new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }));
  const REASONS = [
    ['Idle', 'A while without using the portal'],
    ['Time limit', 'Some hours after you signed in'],
    ['Elsewhere', 'Sign out everywhere on another device'],
    ['Access', 'You left the guild or lost the bossing role'],
  ] as const;
</script>

{#if phone}
  <GateCard bar="Session ended">
    <PhoneNote icon="clock" title={TITLE} hint="Nothing of your account was kept on this device.">
      Sessions end for one of these reasons:
      {#snippet more()}
        <ul class="account-grp ended-reasons" data-fid="ended-reasons">
          {#each REASONS as [cap, words] (cap)}
            <li class="account-row ended-reasons__row"><span class="cap ended-reasons__cap">{cap}</span><span>{words}</span></li>
          {/each}
        </ul>
      {/snippet}
      {#snippet actions()}<a class="btn btn--primary btn--key btn--full" href={discordStart(next)}>Sign in again</a>{/snippet}
    </PhoneNote>
  </GateCard>
{:else}
  <GateWindow {identity} bar="Signed out" {zone}>
    <p class="flash flash--warn" role="status">
      <Icon name="clock" />
      <span><b>Your session ended at <time class="mono" datetime={new Date(at).toISOString()}>{time}</time>.</b> Nothing you'd saved was lost.</span>
    </p>
    <h1 class="gate__lead" id="gate-lead">{TITLE}</h1>
    <p>Sessions end after a while without use or some hours after sign-in, when you sign out elsewhere, or if you leave the guild or lose the bossing role.</p>
    <a class="btn btn--primary btn--key gate__key" href={discordStart(next)} data-fid="gate-key"><Icon name="log-in" />Sign in again</a>
  </GateWindow>
{/if}
