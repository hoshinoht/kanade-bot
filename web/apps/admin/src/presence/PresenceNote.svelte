<!--
  The one sentence near the action when someone else is editing the same thing
  (B_PresenceInbox's decision card, B_PresenceRunSheet's strip under the title
  bar): what the existing version check will do. Never a lock; it disables
  nothing. Nothing when nobody else is editing.
-->
<script lang="ts">
  import type { PresenceKind } from '@kanade/api-types';
  import '@kanade/ui/styles/screens/presence.scss';
  import PresenceAvatar from './PresenceAvatar.svelte';
  import { editingNote, editors, type Shown } from './presence';

  let { list, kind, variant }: { list: Shown[]; kind: PresenceKind; variant: 'card' | 'strip' } = $props();
  const note = $derived(editingNote(kind, list, false));
  const first = $derived(editors(list)[0]);
</script>

{#if note && first}
  <p class="presence-note presence-note--{variant}" data-fid="presence-note"><PresenceAvatar entry={first} /><span><b>{note.lead}</b> {note.rule}</span></p>
{/if}
