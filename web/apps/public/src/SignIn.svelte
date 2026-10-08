<!--
  Signed out (D4-A, board `Main`): the admin login's window (`_gate.scss`)
  with the bot's identity on a colourway banner, one sentence of purpose,
  "Sign in with Discord" and the privacy box (oauth-security item 20). No
  schedule, no counts, no boss art (the portal serves none to visitors). A
  notice says why it shows when there is a reason.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import { Icon, initial } from '@kanade/ui';
  import { discordStart } from './landing';
  import type { SignInNotice } from './portal.svelte';

  let { identity, notice = null, next }: { identity: Identity | null; notice?: SignInNotice | null; next: string } = $props();
  const name = $derived(identity?.name ?? 'Kanade');

  const told = $derived.by(() => {
    if (notice === 'failed') return { tone: 'error', text: "Sign-in didn't finish. Try again in a moment." };
    if (notice === 'switch') return { tone: 'ok', text: 'To use another account, switch to it in Discord first (or sign out of Discord in this browser), then sign in here.' };
    if (notice === 'signed-out') return { tone: 'ok', text: "You're signed out on this device." };
    if (notice) {
      const others = notice.everywhere - 1;
      return { tone: 'ok', text: others > 0 ? `You're signed out everywhere: this device and ${others} other${others === 1 ? '' : 's'}.` : "You're signed out everywhere." };
    }
    return null;
  });
</script>

<div class="gate">
  <section class="gate__window" aria-labelledby="signin-title">
    <div class="gate__bar"><h1 class="gate__bar-title" id="signin-title">Sign in</h1></div>
    <div class="gate__hero gate__hero--motif">
      <div class="gate__id">
        {#if identity}
          <img class="gate__avatar" src={identity.avatar} alt="" width="64" height="64" />
        {:else}
          <span class="gate__avatar" aria-hidden="true">{initial(name)}</span>
        {/if}
        <div class="gate__who">
          <p class="gate__name">{name}</p>
          <p class="gate__sub">the guild's boss schedule</p>
        </div>
      </div>
    </div>
    <div class="gate__body">
      {#if told}
        <p class="flash flash--{told.tone}" role={told.tone === 'error' ? 'alert' : 'status'}>{told.text}</p>
      {/if}
      <p class="gate__lede">Sign in with Discord to see the boss week and manage your own runs. Only members of the guild with the bossing role can get in.</p>
      <!-- A full-page navigation: Discord's consent page and its redirect back need the browser, not fetch. -->
      <a class="btn btn--primary btn--key gate__key" href={discordStart(next)}
        ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
          ><path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3" /></svg
        >Sign in with Discord</a
      >
      <p class="infobox">
        <Icon name="info" />
        <span>
          We ask Discord only who you are (the <code>identify</code> scope). We keep your Discord id, display name and avatar, not your email, and never post
          as you.
        </span>
      </p>
    </div>
  </section>
</div>
