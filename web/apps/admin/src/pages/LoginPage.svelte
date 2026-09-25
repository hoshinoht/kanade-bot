<script lang="ts">
  import '@kanade/ui/styles/gate.scss';
  import type { Identity } from '@kanade/api-types';

  let { identity, onsignin }: { identity: Identity | null; onsignin: () => void } = $props();
  const uid = $props.id();
  const name = $derived(identity?.name ?? 'Kanade');
</script>

<!-- v4 login.html: one window on an empty desktop, wearing the bot's banner and avatar. -->
<div class="gate">
  <section class="gate__window" aria-labelledby="{uid}-name">
    <div class="gate__bar"><span class="gate__bar-title">Sign in</span></div>
    {#if identity}<img class="gate__hero" src={identity.banner} alt="" />{:else}<div class="gate__hero"></div>{/if}
    <div class="gate__body">
      {#if identity}
        <img class="gate__avatar" src={identity.avatar} alt="" width="64" height="64" />
      {:else}
        <span class="gate__avatar" aria-hidden="true">{name.slice(0, 1)}</span>
      {/if}
      <h1 class="gate__name" id="{uid}-name">{name}</h1>
      <p class="gate__sub">
        boss scheduler ·
        <a href="https://github.com/hoshinoht/kanade-bot" rel="noopener noreferrer" target="_blank">powered by kanade</a>
      </p>
      <!-- Admin sign-in design (auth is not wired yet): Discord OAuth first, the
           tailnet identity as fallback, the admin token as break-glass only. -->
      <button class="btn btn--primary gate__primary" type="button" onclick={onsignin}>Sign in with Discord</button>
      <p class="gate__or">
        On the tailnet? Your Tailscale identity signs you in when Discord is unavailable.
      </p>
      <details class="gate__glass">
        <summary>Break-glass: admin token</summary>
        <form
          onsubmit={(event) => {
            event.preventDefault();
            onsignin();
          }}
        >
          <p>For when Discord and the tailnet are both down. Every use is recorded in History.</p>
          <label>
            <span class="label">Admin token</span>
            <input type="password" name="token" autocomplete="current-password" />
          </label>
          <button class="btn" type="submit">Sign in with the token</button>
        </form>
      </details>
      <p class="note">Sign-in is not connected in this build; each option just opens the portal.</p>
    </div>
  </section>
</div>

<style>
  .gate__primary {
    width: 100%;
    justify-content: center;
    margin-top: 0.4rem;
  }

  .gate__or {
    margin: 0.7rem 0 0.3rem;
  }

  .gate__glass summary {
    cursor: pointer;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }
</style>
