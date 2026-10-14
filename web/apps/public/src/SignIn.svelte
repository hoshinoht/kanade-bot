<!--
  Signed out (boards SignIn and its -Expired, -Failed, -Limited,
  -Unavailable variants; phone PhoneGate): the gate window with one sentence
  of purpose, "Sign in with Discord" and the privacy box (oauth-security item
  20). No schedule, no counts, no boss art. A notice says why it shows when
  there is a reason; when member data is unavailable the window says only
  that, since nobody was refused.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import { Icon, initial } from '@kanade/ui';
  import GateCard from './GateCard.svelte';
  import GateWindow from './GateWindow.svelte';
  import PhoneNote from './PhoneNote.svelte';
  import { discordStart } from './landing';
  import type { SignInNotice } from './portal.svelte';

  let {
    identity,
    notice = null,
    next,
    zone,
    phone,
  }: { identity: Identity | null; notice?: SignInNotice | null; next: string; zone: string; phone: boolean } = $props();
  const name = $derived(identity?.name ?? 'Kanade');
  const start = $derived(discordStart(next));

  const told = $derived.by((): { tone: 'ok' | 'warn' | 'error'; lead: string; text: string } | null => {
    if (notice === 'failed') return { tone: 'error', lead: "Sign-in didn't finish.", text: 'Discord or Kanade had a problem. Try again in a moment.' };
    if (notice === 'expired')
      return { tone: 'warn', lead: 'That sign-in took too long.', text: 'The return link from Discord lasts 10 minutes. Start again; it only takes a moment.' };
    if (notice === 'limited') return { tone: 'warn', lead: 'Too many sign-in tries.', text: 'For safety, this device has to wait a few minutes before trying again.' };
    if (notice === 'switch')
      return { tone: 'ok', lead: 'To use another account,', text: 'switch to it in Discord first (or sign out of Discord in this browser), then sign in here.' };
    if (notice === 'signed-out') return { tone: 'ok', lead: "You're signed out on this device.", text: '' };
    if (notice === 'unavailable' || notice === null) return null;
    const others = notice.everywhere - 1;
    return { tone: 'ok', lead: others > 0 ? `You're signed out everywhere: this device and ${others} other${others === 1 ? '' : 's'}.` : "You're signed out everywhere.", text: '' };
  });
  const keyWords = $derived(notice === 'failed' ? 'Try again with Discord' : notice === 'expired' ? 'Start again with Discord' : 'Sign in with Discord');
</script>

{#snippet flash()}
  {#if told}
    <p class="flash flash--{told.tone}" role={told.tone === 'error' ? 'alert' : 'status'} data-fid="gate-notice">
      <Icon name={told.tone === 'ok' ? 'check' : 'alert-circle'} />
      <span><b>{told.lead}</b>{told.text ? ` ${told.text}` : ''}</span>
    </p>
  {/if}
{/snippet}

{#snippet purpose()}
  <p class="gate__lede">Sign in with Discord to see the boss week and manage your own runs. Only members of the guild with the bossing role can get in.</p>
{/snippet}

{#snippet key(full: boolean)}
  <!-- A full-page navigation: Discord's consent page and its redirect back need the browser, not fetch. -->
  {#if notice === 'limited'}
    <div class="gate__keywrap">
      <a class="btn btn--primary btn--key gate__key" class:btn--full={full} href={start} aria-describedby="gate-wait" data-fid="gate-key"
        ><Icon name="log-in" />{keyWords}</a
      >
      <span class="field__hint" id="gate-wait">Wait a few minutes, then try again.</span>
    </div>
  {:else}
    <a class="btn btn--primary btn--key gate__key" class:btn--full={full} href={start} data-fid="gate-key"><Icon name="log-in" />{keyWords}</a>
  {/if}
{/snippet}

{#snippet privacy()}
  <p class="infobox">
    <Icon name="info" />
    <span>
      We ask Discord only who you are (the <code>identify</code> scope). We keep your Discord id, display name and avatar, not your email, and never post
      as you.
    </span>
  </p>
{/snippet}

{#if phone}
  {#if notice === 'unavailable'}
    <GateCard bar="Sign in">
      <PhoneNote icon="alert-circle" error title="Sign-in unavailable, try again" hint="Usually back within a few minutes.">
        Kanade can't read the guild's member list right now, so it can't let anyone in. This isn't about your account.
        {#snippet actions()}<a class="btn btn--primary btn--key btn--full" href={start}>Try again</a>{/snippet}
      </PhoneNote>
    </GateCard>
  {:else}
    <GateCard bar="Sign in" fill={false}>
      <div class="gate-card__hero">
        {#if identity}<img class="gate-card__banner" src={identity.banner} alt="" />{/if}
        {#if identity}
          <img class="gate-card__tile" src={identity.avatar} alt="" width="56" height="56" />
        {:else}
          <span class="gate-card__tile" aria-hidden="true">{initial(name)}</span>
        {/if}
        <span class="gate-card__who">
          <span class="gate-card__name">{name}</span>
          <span class="gate-card__sub">boss schedule</span>
        </span>
      </div>
      <div class="gate-card__body">
        <h1 class="gate__lead" id="gate-lead">Sign in to see the boss week</h1>
        {@render flash()}
        {@render purpose()}
        {@render key(true)}
        {@render privacy()}
      </div>
    </GateCard>
  {/if}
{:else}
  <GateWindow {identity} bar="Sign in" {zone} short={notice === 'unavailable'}>
    {#if notice === 'unavailable'}
      <p class="flash flash--error" role="alert">
        <Icon name="alert-circle" />
        <span><b>Sign-in unavailable, try again.</b> Kanade can't read the guild's member list right now, so it can't check who may sign in.</span>
      </p>
      <h1 class="gate__lead" id="gate-lead">This isn't about your account</h1>
      <p>Nothing was refused. Usually back within a few minutes; Discord works as normal meanwhile.</p>
      <a class="btn btn--primary btn--key gate__key" href={start} data-fid="gate-key">Try again</a>
    {:else}
      {@render flash()}
      <h1 class="gate__lead" id="gate-lead">Sign in to see the boss week</h1>
      {@render purpose()}
      {@render key(false)}
      {@render privacy()}
    {/if}
  </GateWindow>
{/if}
