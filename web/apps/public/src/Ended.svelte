<!--
  A session that ended while in use (`401 unauthenticated` mid-session; board
  `P_States` "Session ended"): it timed out, was signed out from another
  device, or the member is no longer eligible. Nothing of the account stays on
  screen. The time-outs are server settings the portal is not told, so the
  text names them without numbers.
-->
<script lang="ts">
  import { StateNote } from '@kanade/ui';
  import { discordStart } from './landing';

  let { next }: { next: string } = $props();
</script>

<div class="gate">
  <section class="card notice" aria-labelledby="ended-bar">
    <div class="card__head"><span class="card__title" id="ended-bar">Session ended</span></div>
    <StateNote level={1} title="You've been signed out">
      Sessions end after a while without use or some hours after sign-in, when you sign out elsewhere, or if you leave the guild or lose the bossing role.
      {#snippet actions()}
        <a class="btn btn--primary btn--key" href={discordStart(next)}>Sign in again</a>
      {/snippet}
    </StateNote>
  </section>
</div>
