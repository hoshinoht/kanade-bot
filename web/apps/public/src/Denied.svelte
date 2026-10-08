<!--
  `/?login_error=not_eligible` (board `P_Denied`): neutral on purpose. It never
  says whether the account is in the guild or which role is missing, and no
  session exists (the server wrote no row and no cookie). One bordered panel,
  no title bar, as the board.
-->
<script lang="ts">
  import { StateNote } from '@kanade/ui';
  import { discordStart } from './landing';

  let { next, onswitch }: { next: string; onswitch: () => void } = $props();
</script>

<div class="gate">
  <section class="notice notice--bare">
    <StateNote icon="lock" level={1} title="This account can't see the schedule">
      The schedule is for guild members with the bossing role. If you should have access, ask an admin, then try again.
      {#snippet actions()}
        <button type="button" class="btn btn--primary btn--key" onclick={onswitch}>Use another account</button>
        <a class="btn" href={discordStart(next)}>Try again</a>
      {/snippet}
    </StateNote>
  </section>
</div>
