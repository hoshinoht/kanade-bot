<script lang="ts">
  import '@kanade/ui/styles/gate.scss';
  import type { Identity, Session, SignInMethods, Tonight, TonightRun } from '@kanade/api-types';
  import { initial, PendingLabel } from '@kanade/ui';
  import { tick } from 'svelte';
  import { discordStart, loginErrorText } from '../auth';
  import { send } from '../resource.svelte';
  import { artUrl } from '../shared/identity';

  let {
    next,
    loginError = '',
    onsignedin,
  }: {
    /** Where to return once signed in (already a safe same-origin path). */
    next: string;
    /** The Discord callback's `login_error` code, if it sent one. */
    loginError?: string;
    onsignedin: (session: Session) => void;
  } = $props();
  const uid = $props.id();

  let identity = $state<Identity | null>(null);
  let methods = $state<SignInMethods | null>(null);
  let methodsError = $state('');
  /** Today's next run, if any; the server decides "today" in the guild zone. */
  let tonight = $state<TonightRun | null>(null);
  const tonightLabel = $derived(tonight && tonight.time >= '17:00' ? 'Tonight' : 'Today');
  const name = $derived(identity?.name ?? 'Kanade');
  $effect(() => {
    void send((c) => c.get<Identity>('/api/identity')).then((r) => (identity = r.ok ? r.value : null));
    // Decoration only: a failure renders nothing and never holds up sign-in.
    void send((c) => c.get<Tonight>('/api/admin/auth/tonight')).then((r) => (tonight = r.ok ? r.value.run : null));
    void send((c) => c.get<SignInMethods>('/api/admin/auth/methods')).then((r) => {
      if (r.ok) {
        methods = r.value;
        // The token is the only way in: show its form rather than a closed disclosure.
        glassOpen = r.value.token && !r.value.discord && !r.value.tailscale;
      } else methodsError = `Can't reach Kanade to sign in: ${r.message}`;
    });
  });

  let glassOpen = $state(false);
  let token = $state('');
  let tokenError = $state('');
  let busy = $state(false);
  /** The sign-in path in flight, so only its button shows it. */
  let signingIn = $state('');
  let tokenInput = $state<HTMLInputElement>();
  const TOKEN_REFUSED: Record<number, string> = {
    400: 'Enter the admin token.',
    401: "That token isn't right.",
    429: 'Too many attempts. Wait a minute, then try again.',
  };

  const TAILSCALE_REFUSED: Record<number, string> = {
    401: "Your tailnet identity isn't on Kanade's admin list.",
    429: 'Too many attempts. Wait a minute, then try again.',
  };

  async function signIn(path: string, body: object, refusals: Record<number, string>, onError: (message: string) => void) {
    busy = true;
    signingIn = path;
    const result = await send((c) => c.post<Session>(path, body));
    busy = false;
    signingIn = '';
    if (result.ok) onsignedin(result.value);
    else onError((result.status && refusals[result.status]) || result.message);
  }

  function submitToken(event: SubmitEvent) {
    event.preventDefault();
    if (!token) {
      tokenError = TOKEN_REFUSED[400]!;
      tokenInput?.focus();
      return;
    }
    // The token is sent once and never kept, logged or shown.
    void signIn('/api/admin/auth/token', { token }, TOKEN_REFUSED, (message) => {
      tokenError = message;
      token = '';
      void tick().then(() => tokenInput?.focus());
    });
  }
</script>

<!-- One window on an empty desktop (HeroLogin): the bot's banner under a scrim, its avatar and name on it. -->
<div class="gate">
  <section class="gate__window" aria-labelledby="{uid}-name" data-fid="gate">
    <div class="gate__bar" data-fid="gate-bar"><span class="gate__bar-title">Sign in</span></div>
    <div class="gate__hero" data-fid="gate-hero">
      {#if identity}<img class="gate__banner" src={artUrl(identity.banner, identity)} alt="" />{/if}
      <div class="gate__id" data-fid="gate-id">
        {#if identity}
          <img class="gate__avatar" src={artUrl(identity.avatar, identity)} alt="" width="76" height="76" data-fid="gate-avatar" />
        {:else}
          <span class="gate__avatar" aria-hidden="true" data-fid="gate-avatar">{initial(name)}</span>
        {/if}
        <div class="gate__who" data-fid="gate-name">
          <h1 class="gate__name" id="{uid}-name">{name}</h1>
          <p class="gate__sub">
            boss scheduler ·
            <a href="https://github.com/hoshinoht/kanade-bot" rel="noopener noreferrer" target="_blank">powered by kanade</a>
          </p>
        </div>
      </div>
    </div>
    <div class="gate__body" data-fid="gate-body">
      {#if tonight}
        <p class="gate__tonight" data-fid="gate-tonight">
          <span class="cap">{tonightLabel}</span>
          <span class="gate__tonight-time">{tonight.time}</span>
          <span class="gate__tonight-bosses">{tonight.bosses.join(' + ')}</span>
          <span class="gate__tonight-tally"><span class="vh">answered yes:</span> {tonight.tally.on}/{tonight.tally.total}</span>
        </p>
      {/if}
      <!-- Discord OAuth first, the tailnet identity as fallback, the admin token
           as break-glass only; each shown only when this server offers it. -->
      {#if loginError}<p class="flash flash--error" role="alert">{loginErrorText(loginError)}</p>{/if}
      {#if methodsError}<p class="flash flash--error" role="alert">{methodsError}</p>{/if}
      {#if !methods && !methodsError}<p class="note" aria-busy="true">Checking how you can sign in…</p>{/if}
      {#if methods?.discord}
        <!-- A full-page navigation: Discord's consent page and its redirect back need the browser, not fetch. -->
        <a class="btn btn--primary btn--key gate__primary gate__key" href={discordStart(next)} data-fid="gate-key"
          ><svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"
            ><path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4M10 17l5-5-5-5M15 12H3" /></svg
          >Sign in with Discord</a
        >
      {/if}
      {#if methods?.tailscale}
        <p class="gate__or">On the tailnet? Your Tailscale identity signs you in when Discord is unavailable.</p>
        <button
          class="btn gate__primary"
          type="button"
          disabled={busy}
          onclick={() => void signIn('/api/admin/auth/tailscale', {}, TAILSCALE_REFUSED, (message) => (methodsError = message))}
          ><PendingLabel pending={signingIn === '/api/admin/auth/tailscale'} label="Signing in…">Sign in with Tailscale</PendingLabel></button
        >
      {/if}
      {#if methods?.token}
        <details class="gate__glass" bind:open={glassOpen} data-fid="gate-glass">
          <summary>Break-glass: admin token</summary>
          <form onsubmit={submitToken} novalidate>
            <p>For when Discord and the tailnet are both down. Every use is recorded in History.</p>
            <label>
              <span class="label">Admin token</span>
              <input
                type="password"
                name="token"
                autocomplete="current-password"
                bind:value={token}
                bind:this={tokenInput}
                aria-invalid={tokenError ? 'true' : undefined}
                aria-describedby="{uid}-token-err"
              />
            </label>
            <p class="field__error" id="{uid}-token-err" role="alert">{tokenError}</p>
            <button class="btn" type="submit" disabled={busy}><PendingLabel pending={signingIn === '/api/admin/auth/token'} label="Signing in…">Sign in with the token</PendingLabel></button>
          </form>
        </details>
      {/if}
      {#if methods && !methods.discord && !methods.tailscale && !methods.token}
        <p class="note">Sign-in is not configured on this server.</p>
      {/if}
    </div>
  </section>
</div>

<style>
  .gate__or {
    margin: 0;
  }

  .gate__glass {
    font-size: var(--fs-small);
  }

  .gate__glass summary {
    min-height: 1.5rem;
    padding-block: 0.2rem;
    cursor: pointer;
    font-size: var(--fs-small);
    color: var(--dim-text);
  }
</style>
