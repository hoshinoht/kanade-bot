<!--
  A phone's signed-out note (boards PhoneClosed, PhoneDenied, PhoneEnded,
  PhoneGate-Unavailable): the shared state-note look (`_state-note.scss`),
  centred in the window, with room for who signed in (`lead`), a list under
  the text (`more`), the actions full width, and a hint under them.
-->
<script lang="ts">
  import { Icon, type IconName } from '@kanade/ui';
  import type { Snippet } from 'svelte';

  let {
    icon,
    title,
    error = false,
    lead,
    children,
    more,
    actions,
    hint = '',
  }: {
    icon: IconName;
    title: string;
    /** A failure (sign-in unavailable): the smaller glyph on a risk wash. */
    error?: boolean;
    lead?: Snippet;
    children: Snippet;
    more?: Snippet;
    actions: Snippet;
    hint?: string;
  } = $props();
</script>

<div class="state-pane gate-card__pane" data-fid="state-pane">
  <div class="state-note" class:state-note--error={error} data-fid="state-note">
    <span class="state-note__glyph" aria-hidden="true" data-fid="state-glyph"><Icon name={icon} /></span>
    <h1 class="state-note__title" id="gate-lead">{title}</h1>
    {@render lead?.()}
    <p class="state-note__text" data-fid="state-text">{@render children()}</p>
    {@render more?.()}
    <div class="state-note__actions gate-card__actions" data-fid="state-actions">{@render actions()}</div>
    {#if hint}<p class="field__hint">{hint}</p>{/if}
  </div>
</div>
