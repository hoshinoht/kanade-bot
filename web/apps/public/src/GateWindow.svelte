<!--
  The signed-out window on a wide screen (boards SignIn, Closed, Denied,
  Ended): the admin login's window (`_gate.scss`) with its title bar, the
  bot's banner, avatar and name, the screen's own body, and the foot naming
  the time zone. The banner is the bot's (`/identity/banner`), never boss
  art: the portal serves no art to visitors. Until the identity answers, the
  colourway's dotted motif stands in.
-->
<script lang="ts">
  import type { Identity } from '@kanade/api-types';
  import { initial } from '@kanade/ui';
  import type { Snippet } from 'svelte';

  let {
    identity,
    bar,
    zone,
    short = false,
    muted = false,
    children,
  }: {
    identity: Identity | null;
    /** The title bar's words ("Sign in", "Closed"). */
    bar: string;
    zone: string;
    /** A shorter banner where the body has more to say (Closed, Not eligible). */
    short?: boolean;
    /** Greyed while the portal is closed. */
    muted?: boolean;
    /** The body; its h1 has the id `gate-lead`. */
    children: Snippet;
  } = $props();
  const name = $derived(identity?.name ?? 'Kanade');
</script>

<div class="gate">
  <section class="gate__window" aria-labelledby="gate-lead" data-fid="gate">
    <div class="gate__bar" data-fid="gate-bar"><span class="gate__bar-title">{bar}</span></div>
    <div class="gate__hero" class:gate__hero--short={short} class:gate__hero--muted={muted} class:gate__hero--motif={!identity} data-fid="gate-hero">
      {#if identity}<img class="gate__banner" src={identity.banner} alt="" />{/if}
      <div class="gate__id" data-fid="gate-id">
        {#if identity}
          <img class="gate__avatar" src={identity.avatar} alt="" width="76" height="76" data-fid="gate-avatar" />
        {:else}
          <span class="gate__avatar" aria-hidden="true" data-fid="gate-avatar">{initial(name)}</span>
        {/if}
        <span class="gate__who">
          <span class="gate__name" data-fid="gate-name">{name}</span>
          <span class="gate__sub">the guild's boss schedule</span>
        </span>
      </div>
    </div>
    <div class="gate__body" data-fid="gate-body">
      {@render children()}
    </div>
    <div class="gate__foot" data-fid="gate-foot"><span>Times in {zone}</span></div>
  </section>
</div>
