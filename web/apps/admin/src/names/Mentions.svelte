<!--
  Message text with Discord mention tokens shown as @name / #channel /
  @role (the shared Name, so each still copies its id).
-->
<script lang="ts">
  import { directory } from './directory.svelte';
  import { leadingMember, parseMentions } from './mentions';
  import Name from './Name.svelte';

  let {
    text,
    plain = false,
    asked = false,
    dropBot = false,
  }: {
    text: string;
    /** Inside a link: names as text, no copy buttons. */
    plain?: boolean;
    /** A chatbot question, which opens by mentioning Kanade. */
    asked?: boolean;
    /** Leave out the opening bot mention: every question is asked of Kanade, so it says nothing. */
    dropBot?: boolean;
  } = $props();

  const segments = $derived.by(() => {
    const all = parseMentions(text.trimStart());
    const first = all[0];
    const bot = first?.kind === 'member' && (first.id === directory.bot.id || !directory.members.has(first.id));
    if (!(asked && dropBot && bot)) return all;
    const rest = all.slice(1);
    if (rest[0]?.kind === 'text') rest[0] = { kind: 'text', text: rest[0].text.trimStart() };
    return rest;
  });
  $effect(() => {
    const bot = asked ? leadingMember(text) : null;
    if (bot) directory.noteBot(bot);
  });
</script>

{#each segments as s, i (i)}{#if s.kind === 'text'}{s.text}{:else}<Name kind={s.kind} id={s.id} mention {plain} />{/if}{/each}
