<!--
  Every core primitive (web/packages/ui/src/styles/core/) once, for visual
  checks across colourways: tokens and contrast pairs, buttons, chips, forms,
  tabs, connected groups, windows, the modal, list and side panes, toasts, the
  fixed frame with its rail and page line, the phone top bar, and motion and
  loading. Screen partials are deliberately absent.
-->
<script lang="ts">
  import {
    AccountMenu,
    Freshness,
    Icon,
    ListPane,
    LoadError,
    LoadingIndicator,
    LoadingState,
    Modal,
    PendingLabel,
    Select,
    SidePane,
    StateNote,
    StatusChip,
    Tabs,
    ThemeTiles,
    ThreadPanel,
    Toaster,
    ToastRegion,
    WavyProgress,
  } from '@kanade/ui';
  import PageLine from '../../../web/apps/admin/src/shell/PageLine.svelte';
  import Rail from '../../../web/apps/admin/src/shell/Rail.svelte';
  import TopBar from '../../../web/apps/admin/src/shell/TopBar.svelte';
  import { setChrome } from '../../../web/apps/admin/src/shell/chrome';

  // Token swatches: fills with the ink that sits on them.
  const SWATCHES = ['ground', 'surface', 'raise', 'pane', 'row', 'win', 'accent-fill', 'select', 'seg-fill', 'chip-fill', 'line', 'ok', 'warn', 'risk', 'own', 'off'];
  // Text pairs the contrast overrides (tokens/_contrast.scss) keep at 4.5:1.
  const PAIRS = [
    ['ground-ink', 'ground'],
    ['ink', 'surface'],
    ['dim-text', 'surface'],
    ['win-ink', 'win'],
    ['accent-ink', 'accent-fill'],
    ['select-ink', 'select'],
    ['accent-text', 'surface'],
    ['ok-text', 'surface'],
    ['warn-text', 'surface'],
    ['risk-text', 'surface'],
  ] as const;

  setChrome({ phone: false, fresh: 'live', updated: '12:00', timezone: 'Europe/London', quiet: false, palette() {}, back() {} });

  const toaster = new Toaster();
  toaster.show({ message: 'Moved HCarling to Thursday 21:00.', action: { label: 'Undo', run: () => {} }, timeoutMs: null });

  let tab = $state<'party' | 'answers' | 'changes'>('party');
  let group = $state('week');
  let style = $state('plain');
  let pick = $state('kalos');
  let modal = $state(false);
  let pending = $state(true);
  let selected = $state('r2');
  const rows = [
    { id: 'r1', title: 'HCarling + HStar', meta: 'Thu 21:00 · 6 of 6' },
    { id: 'r2', title: 'Chaos Kalos', meta: 'Fri 21:30 · 4 of 6' },
    { id: 'r3', title: 'Hard Limbo', meta: 'Sat 20:00 · 2 of 6' },
  ];
</script>

<main class="showcase">
  <header class="showcase__head">
    <h1>Core primitives</h1>
    <p class="note">Dev-only (pwa-mock). Change the colourway below; every section restyles.</p>
  </header>

  <section class="card" aria-labelledby="sc-tokens">
    <header class="card__head"><h2 class="card__title" id="sc-tokens">Tokens, colourways, contrast</h2></header>
    <ThemeTiles />
    <ul class="showcase__swatches">
      {#each SWATCHES as name (name)}
        <li><span class="showcase__swatch showcase__swatch--{name}"></span><span class="mono">--{name}</span></li>
      {/each}
    </ul>
    <ul class="showcase__pairs">
      {#each PAIRS as [ink, fill] (ink + fill)}
        <li class="showcase__pair showcase__pair--{ink}-on-{fill}">--{ink} on --{fill}</li>
      {/each}
    </ul>
  </section>

  <section class="card" aria-labelledby="sc-buttons">
    <header class="card__head"><h2 class="card__title" id="sc-buttons">Buttons</h2></header>
    <div class="showcase__row">
      <button type="button" class="btn">Refresh</button>
      <button type="button" class="btn btn--primary">Save</button>
      <button type="button" class="btn btn--key"><Icon name="check" /> Approve</button>
      <button type="button" class="btn btn--danger">Reject…</button>
      <button type="button" class="btn btn--risk">Revert</button>
      <button type="button" class="btn btn--ghost">Cancel</button>
      <button type="button" class="btn" disabled>Undo</button>
      <span class="kbd">Ctrl K</span>
      <button type="button" class="linklike">Copy details</button>
    </div>
  </section>

  <section class="card" aria-labelledby="sc-chips">
    <header class="card__head"><h2 class="card__title" id="sc-chips">Chips and status</h2></header>
    <div class="showcase__row">
      <span class="chip">Mika</span>
      <span class="chip">Ren<button type="button" class="chip__x" aria-label="Remove Ren">×</button></span>
      <button type="button" class="chip__add">+ Add</button>
      <StatusChip>Waiting</StatusChip>
      <StatusChip tone="ok">Confirmed</StatusChip>
      <StatusChip tone="warn">Expired</StatusChip>
      <StatusChip tone="risk">At risk</StatusChip>
      <span class="tone tone--neutral">Settled</span>
      <span class="mchip"><Freshness state="live" updated="12:00" /></span>
      <span class="id">1543532497948909578</span>
    </div>
  </section>

  <section class="card" aria-labelledby="sc-forms">
    <header class="card__head"><h2 class="card__title" id="sc-forms">Forms and connected groups</h2></header>
    <div class="showcase__row">
      <label class="field"><span>Boss</span><input value="hstar" size="10" /></label>
      <Select label="Boss" bind:value={pick} options={[{ value: 'kalos', label: 'Chaos Kalos' }, { value: 'limbo', label: 'Hard Limbo' }]} />
      <div class="seg" role="group" aria-label="Week">
        <button type="button" aria-pressed={group === 'week'} onclick={() => (group = 'week')}>This week</button>
        <button type="button" aria-pressed={group === 'next'} onclick={() => (group = 'next')}>Next week</button>
      </div>
      <div class="seg" role="group" aria-label="Reply style">
        <button type="button" class="seg__btn" aria-pressed={style === 'plain'} onclick={() => (style = 'plain')}>Plain</button>
        <button type="button" class="seg__btn" aria-pressed={style === 'warm'} onclick={() => (style = 'warm')}>Warm</button>
      </div>
    </div>
  </section>

  <section class="card" aria-labelledby="sc-tabs">
    <header class="card__head"><h2 class="card__title" id="sc-tabs">Tabs</h2></header>
    <Tabs
      label="Run"
      bind:selected={tab}
      items={[
        { id: 'party', label: 'Party', count: 6 },
        { id: 'answers', label: 'Answers', count: 2 },
        { id: 'changes', label: 'Changes' },
      ]}
    >
      {#snippet panel(id)}<p class="note">The {id} panel.</p>{/snippet}
    </Tabs>
  </section>

  <section class="card" aria-labelledby="sc-modal">
    <header class="card__head"><h2 class="card__title" id="sc-modal">Modal</h2></header>
    <button type="button" class="btn" onclick={() => (modal = true)}>Open the modal</button>
    <Modal bind:open={modal} title="Revert #2?" eyebrow="History">
      <p>Puts HCarling back on Thursday 20:00 and tells its party.</p>
      {#snippet footer(close)}
        <button type="button" class="btn btn--ghost" onclick={close}>Cancel</button>
        <button type="button" class="btn btn--risk" onclick={close}>Revert</button>
      {/snippet}
    </Modal>
    <div class="empty"><strong>Nothing waiting</strong>New proposals appear here.</div>
  </section>

  <section class="card" aria-labelledby="sc-states">
    <header class="card__head"><h2 class="card__title" id="sc-states">Motion, loading and states</h2></header>
    <div class="showcase__row">
      <LoadingIndicator />
      <LoadingIndicator size="sm" />
      <button type="button" class="btn" onclick={() => (pending = !pending)}><PendingLabel {pending} label="Saving">Save</PendingLabel></button>
    </div>
    <WavyProgress value={7} max={12} label="Answers" text="7 of 12" />
    <WavyProgress value={3} max={12} label="Answers (flat)" text="3 of 12" wavy={false} />
    <WavyProgress value={11} max={12} label="Permits" text="11 of 12" tone="warn" ticks={[0.5]} />
    <div class="showcase__states">
      <LoadingState text="Loading the week…" />
      <StateNote icon="inbox" title="Inbox zero"><p>Nothing needs a decision.</p></StateNote>
      <LoadError thing="the week" reason="The server did not answer." onretry={() => {}} level={3} />
    </div>
  </section>

  <h2 class="showcase__label">Fixed frame: rail, page line, list and side panes, thread</h2>
  <div class="frame frame--rail showcase__stage">
    <Rail name="Kanade" avatar={null} active="inbox" inbox={3}>
      {#snippet account()}
        <AccountMenu who="Asahi" detail="Discord">
          {#snippet items(hide)}<button type="button" role="menuitem" tabindex="-1" class="account__item" onclick={() => hide()}>Sign out</button>{/snippet}
        </AccountMenu>
      {/snippet}
    </Rail>
    <div class="shell">
      <PageLine title="Inbox"><h2 class="showcase__pageline-h">3 waiting</h2></PageLine>
      <section class="card showcase__window" aria-label="Inbox">
        <header class="card__head"><h3 class="card__title">Inbox</h3></header>
        <div class="showcase__panes">
          <ListPane
            label="Items"
            activeDescendant="sc-{selected}"
            onclick={(event) => {
              const option = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-row]') : null;
              if (option?.dataset.row) selected = option.dataset.row;
            }}
          >
            {#each rows as row (row.id)}
              <li id="sc-{row.id}" role="option" aria-selected={row.id === selected} class="showcase__option" data-row={row.id}>
                <strong>{row.title}</strong><span class="note">{row.meta}</span>
              </li>
            {/each}
          </ListPane>
          <ThreadPanel label="Thread"><p>Mika: can we move Kalos to 21:30?</p></ThreadPanel>
          <SidePane label="Details"><dl><dt>Run</dt><dd>Chaos Kalos</dd><dt>When</dt><dd>Fri 21:30</dd></dl></SidePane>
        </div>
      </section>
    </div>
  </div>

  <h2 class="showcase__label">Phone top bar</h2>
  <div class="frame frame--rail frame--phone showcase__stage showcase__stage--phone">
    <TopBar title="Week" open={false} inbox={3} fresh="live" updated="12:00" timezone="Europe/London" drawerId="sc-drawer" onmenu={() => {}} />
    <div class="shell">
      <section class="card" aria-label="Week"><header class="card__head"><h3 class="card__title">Week</h3></header><p class="note">One window fills the phone frame.</p></section>
    </div>
  </div>

  <ToastRegion {toaster} />
</main>
