<!-- Who made a change: a member (or a Discord-signed-in admin) by name, copying their id; others in words. -->
<script lang="ts">
  import type { ChangeRecord } from '@kanade/api-types';
  import Name from '../names/Name.svelte';
  import { actorName, type Names } from './describe';

  let { actor, names, known }: { actor: ChangeRecord['actor']; names: Names; known: (id: string) => boolean } = $props();
  const discord = $derived(actor.kind === 'admin' && actor.id.startsWith('discord:') ? actor.id.slice(8) : null);
</script>

{#if actor.kind === 'member'}<Name kind="member" id={actor.id} name={names(actor.id)} />{:else if discord && known(discord)}<Name
    kind="member"
    id={discord}
    name={names(discord)}
  />{:else}{actorName(actor, names, known)}{/if}
