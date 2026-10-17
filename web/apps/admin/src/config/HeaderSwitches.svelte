<!--
  Notifications: what reminder header rewrites may use (boards B_ConfigHeaders,
  B_ConfigHeadersOn). Estimates first; teasing rides on them, so its switch is
  unavailable while estimates are off, and turning estimates off turns teasing
  off in the same save (the server does both). Turning estimates off and
  teasing on ask first; every toast offers Undo.
-->
<script lang="ts">
  import type { Save } from './save';
  import SwitchCard from './SwitchCard.svelte';

  let { estimates, teasing, save }: { estimates: boolean; teasing: boolean; save: Save } = $props();

  function setEstimates(on: boolean): Promise<string> {
    if (on)
      return save({ notifications: { header_estimates: true } }, 'Estimates in reminder headers are on.', {
        patch: { notifications: { header_estimates: false } },
        done: 'Estimates in reminder headers are off again.',
      });
    // Undo brings teasing back too when this save turned it off.
    if (teasing)
      return save({ notifications: { header_estimates: false } }, 'Estimates in reminder headers are off, and teasing with them.', {
        patch: { notifications: { header_estimates: true, header_teasing: true } },
        done: 'Estimates and teasing in reminder headers are on again.',
      });
    return save({ notifications: { header_estimates: false } }, 'Estimates in reminder headers are off.', {
      patch: { notifications: { header_estimates: true } },
      done: 'Estimates in reminder headers are on again.',
    });
  }

  function setTeasing(on: boolean): Promise<string> {
    return save({ notifications: { header_teasing: on } }, on ? 'Teasing in reminder headers is on.' : 'Teasing in reminder headers is off.', {
      patch: { notifications: { header_teasing: !on } },
      done: on ? 'Teasing in reminder headers is off again.' : 'Teasing in reminder headers is on again.',
    });
  }
</script>

<SwitchCard
  fid="cfg-estimates-headers"
  title={() => 'Estimates in reminder headers'}
  on={estimates}
  confirm="Turn estimate headers off?"
  detail={teasing
    ? 'Headers go back to today’s wording, including lines already written, and teasing goes off too. You can undo it from the message that follows.'
    : 'Headers go back to today’s wording, including lines already written. You can undo it from the message that follows.'}
  action={(next) => (next ? 'Turn estimate headers on' : 'Turn estimate headers off')}
  apply={setEstimates}
>
  {#if estimates}Day-of and countdown headers may use each boss’s party estimate and praise the top two carriers. The chat model gets the figures, never a name;
    Kanade adds the names afterwards.{:else}Headers are worded as today. On, the chat model also gets each boss’s party estimate and the top two carriers’ shares,
    never a name; Kanade adds the names afterwards.{/if}
</SwitchCard>
<SwitchCard
  fid="cfg-tease-headers"
  title={() => 'Teasing in reminder headers'}
  on={teasing}
  disabled={!estimates}
  confirm="Turn teasing on?"
  confirmOn
  detail="Members who allowed it may be teased in headers. You can undo it from the message that follows."
  action={(next) => (next ? 'Turn teasing on' : 'Turn teasing off')}
  apply={setTeasing}
>
  {#if estimates}A header may tease one party member who allows it, from their share and any running-joke note; never their name. Members choose on their portal
    Profile; admins on Members.{:else}Turn on Estimates in reminder headers first. Then a header may tease one party member who allows it, from their share and any
    running-joke note; never their name.{/if}
</SwitchCard>
