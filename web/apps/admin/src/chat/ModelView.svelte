<!--
  A historical masked turn's Model view (admin only, sensitive): the turn as
  the model received it, members under fake names. Collapsed until asked
  for, and rendered only once opened; the warning follows the disclosure.
-->
<script lang="ts">
  import type { ModelView } from '@kanade/api-types';
  import { Icon } from '@kanade/ui';
  import { messageParts } from './facts';

  let { view }: { view: ModelView } = $props();
  const uid = $props.id();
  let open = $state(false);
</script>

<section class="chat-modelview" data-fid="chat-modelview" aria-labelledby="{uid}-title">
  <h3 class="chat-modelview__title" id="{uid}-title">
    <button type="button" class="chat-modelview__toggle" aria-expanded={open} aria-controls="{uid}-body" onclick={() => (open = !open)}
      ><Icon name="chevron-right" /> Model view (masked)</button
    >
  </h3>
  <div class="modelview" id="{uid}-body" hidden={!open}>
    {#if open}
      <h4 class="modelview__head">Name mapping</h4>
      {#if view.mapping.length}
        <table class="modelview__map">
          <caption class="vh">Fake names and the members they stand for</caption>
          <thead><tr><th scope="col">Fake name</th><th scope="col">Member</th></tr></thead>
          <tbody>
            {#each view.mapping as m (m.token)}<tr><th scope="row" class="mono">{m.token}</th><td>{m.name}</td></tr>{/each}
          </tbody>
        </table>
      {:else}<p class="note">No names were replaced.</p>{/if}
      {#each view.rounds as r (r.round)}
        <h4 class="modelview__head">Round {r.round}{#if r.clean}<span class="note"> · clean retry</span>{/if}</h4>
        <h5 class="modelview__label">Request as sent</h5>
        <ol class="modelview__messages">
          {#each r.request as message, i (i)}
            {@const m = messageParts(message)}
            <li>
              <span class="modelview__role mono">{m.role}</span>
              {#if m.content !== null}<pre>{m.content}</pre>{/if}
              {#if m.extra}<pre class="modelview__extra">{m.extra}</pre>{/if}
              {#if m.content === null && !m.extra}<span class="note">— empty —</span>{/if}
            </li>
          {/each}
        </ol>
        <h5 class="modelview__label">Raw reply</h5>
        {#if r.reply !== null}<pre>{r.reply}</pre>{:else}<p class="note">— no text; it asked for tools —</p>{/if}
        {#if r.tool_calls.length}
          <h5 class="modelview__label">Raw tool calls</h5>
          <ul class="modelview__calls">
            {#each r.tool_calls as c, i (i)}<li><span class="mono">{c.name}</span><pre>{c.arguments}</pre></li>{/each}
          </ul>
        {/if}
      {/each}
      <h4 class="modelview__head">Final reply members saw</h4>
      <pre>{view.reply}</pre>
    {/if}
  </div>
</section>
<p class="note chat-modelview__note">Admin only, sensitive: the turn as the model received it, with members under fake names.</p>
