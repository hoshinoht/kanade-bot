<!--
  "Confirm it's you" (boards ConfirmAnswer, RequestForm-Confirm,
  MyRuns-Timings-Owner frame 3): a write the server refused with `401
  reauth_required` saved nothing; Not now closes, Sign in again goes through
  Discord and comes back to `next` with the choice kept (the page reads it
  from the address). The lead says what needs it and when this device signed
  in; the body says what is kept and what to press once more.
-->
<script lang="ts">
  import type { PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { Icon, Modal } from '@kanade/ui';
  import type { Snippet } from 'svelte';
  import { discordStart } from '../landing';
  import OwnIcon from '../timings/OwnIcon.svelte';
  import { freshWindow } from '../timings/ownership';

  let {
    open = $bindable(false),
    what,
    next,
    session,
    current,
    phone,
    hint = '',
    returnFocus,
    children,
  }: {
    open: boolean;
    /** What needs the sign-in: "Changing your answer". */
    what: string;
    /** Where the sign-in comes back to. */
    next: string;
    session: PublicSession;
    current: PublicSessionRow | null;
    phone: boolean;
    /** The line under the box: what does not need it. */
    hint?: string;
    returnFocus?: () => HTMLElement | null;
    /** What is kept and what to press once more. */
    children: Snippet;
  } = $props();

  const fresh = $derived(freshWindow(session, current));
  const freshWords = $derived(fresh ? `a Discord sign-in from the last ${fresh.minutes} minutes` : 'a recent Discord sign-in');
</script>

<!-- Every board draws it as `confirm` (head, body, foot). -->
<Modal bind:open eyebrow="Fresh sign-in" title="Confirm it's you" narrow className={phone ? 'own-sheet' : ''} {returnFocus} fid="confirm">
  <p class="own-dialog__lead">
    {what} needs {freshWords}.{fresh ? ' Yours was at ' : ''}{#if fresh}<span class="mono">{fresh.at}</span>.{/if}
  </p>
  <p class="infobox own-dialog__box"><OwnIcon name="save" /><span>{@render children()}</span></p>
  {#if hint}<p class="field__hint">{hint}</p>{/if}
  {#snippet footer(close)}
    <button type="button" class="btn" onclick={close}>Not now</button>
    <a class="btn btn--primary" href={discordStart(next)}><Icon name="log-in" />Sign in again</a>
  {/snippet}
</Modal>
