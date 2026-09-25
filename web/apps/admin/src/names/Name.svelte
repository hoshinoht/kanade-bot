<!--
  A Discord member, channel or role by name (never the raw id). Activating it
  copies the id; the title names it, and a polite status confirms the copy.
  `plain` renders text only, for places already inside a link or an option.
-->
<script lang="ts">
  import { directory } from './directory.svelte';
  import type { NameKind } from './mentions';

  let {
    kind,
    id,
    name = '',
    mention = false,
    plain = false,
    clip = false,
  }: {
    kind: NameKind;
    id: string;
    /** The server's name for it, used when the lists do not know the id. */
    name?: string | null;
    /** Read as a Discord mention: `@name`. */
    mention?: boolean;
    plain?: boolean;
    /** One line, cut with an ellipsis; the tooltip then leads with the full name. */
    clip?: boolean;
  } = $props();

  const label = $derived(directory.label(kind, id, name ?? '', mention));
  const WHAT: Record<NameKind, string> = { member: 'member', channel: 'channel', role: 'role' };
  let status = $state('');
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await navigator.clipboard.writeText(id);
      status = `Copied the ${WHAT[kind]} ID of ${label}.`;
    } catch {
      status = `Couldn't copy the ID; it is ${id}.`;
    }
    clearTimeout(timer);
    timer = setTimeout(() => (status = ''), 3000);
  }
</script>

{#if plain || !id}<span class="name" class:name--clip={clip} title={clip ? label : undefined}>{label}</span>{:else}<button
    type="button"
    class="name name--copy"
    class:name--clip={clip}
    title="{clip ? `${label} · ` : ''}{WHAT[kind][0]!.toUpperCase()}{WHAT[kind].slice(1)} ID {id} — click to copy"
    onclick={() => void copy()}>{label}</button
  ><span class="vh" role="status">{status}</span>{/if}

<style>
  /* Reads as the text it replaces; only the dotted underline says it acts. */
  .name--copy {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    padding: 0;
    cursor: copy;
    text-decoration: underline dotted;
    text-underline-offset: 0.2em;
  }

  .name--clip {
    display: inline-block;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    vertical-align: bottom;
  }
</style>
