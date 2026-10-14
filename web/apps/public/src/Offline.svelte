<script lang="ts">
  import { Masthead, NapArt } from '@kanade/ui';
</script>

<div class="frame">
  <Masthead name="Kanade" by="boss schedule" />
  <main class="shell" id="main">
    <section class="card window-fill offline" aria-labelledby="offline-title">
      <div class="card__head"><h1 class="card__title" id="offline-title">You're offline</h1></div>
      <div class="offline__body">
        <NapArt />
        <p>Kanade needs a connection to sign you in and show your account; nothing is kept on this device.</p>
        <a class="btn btn--primary" href="/">Try again</a>
      </div>
    </section>
  </main>
</div>

<style>
  .offline {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .offline__body {
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    display: grid;
    justify-items: center;
    align-content: start;
    gap: 0.8rem;
    text-align: center;
  }

  .offline__body p {
    margin: 0;
    max-width: 44ch;
    color: var(--dim);
  }
</style>
