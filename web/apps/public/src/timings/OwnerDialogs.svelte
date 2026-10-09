<!--
  Weekly timings' two dialogs (boards MyRuns-Timings-Owner frames 2–3,
  PhoneMyRuns-Timings-Owner frames 3–4): Hand to… (pick a party member; a
  modal on wide screens, a bottom sheet on phones) and Confirm it's you, when
  a hand-off or an Accept needs a Discord sign-in newer than the server's
  fresh-write window. Signing in again comes back to Weekly timings; a
  hand-off comes back with the picker open on the same member.
-->
<script lang="ts">
  import type { PublicSession, PublicSessionRow } from '@kanade/api-types';
  import { Avatar, Icon, Modal } from '@kanade/ui';
  import { discordStart } from '../landing';
  import type { Route } from '../route.svelte';
  import { returnPath, rowControl, type OwnerFlow } from './flow.svelte';
  import OwnIcon from './OwnIcon.svelte';
  import { expiresWords, freshWindow, tokenWords, whenWords } from './ownership';
  import type { MemberTimingsList } from './timings.svelte';

  let {
    list,
    flow,
    route,
    session,
    current,
    phone,
  }: { list: MemberTimingsList; flow: OwnerFlow; route: Route; session: PublicSession; current: PublicSessionRow | null; phone: boolean } = $props();

  const picking = $derived(flow.picking);
  const confirming = $derived(flow.confirming);
  const fresh = $derived(freshWindow(session, current));
  const freshWords = $derived(fresh ? `a Discord sign-in from the last ${fresh.minutes} minutes` : 'a recent Discord sign-in');
  const to = $derived(picking?.timing.party.find((m) => m.id === picking.to) ?? null);
  // The timing the open dialog is about, kept after it closes so focus can go back to its row.
  let about = '';
  $effect(() => {
    const id = picking?.timing.id ?? confirming?.timing.id;
    if (id) about = id;
  });

  // Back from the fresh sign-in (`?hand=<timing>&to=<member>`): the picker again, on the same member.
  $effect(() => {
    const hand = route.params.get('hand');
    if (!hand || !list.data) return;
    const want = route.params.get('to') ?? '';
    const timing = list.data.timings.find((t) => t.id === hand && t.you_own);
    route.set({ hand: '', to: '' });
    if (timing && timing.party.some((m) => m.id === want && m.id !== timing.owner.id)) flow.pick(timing, want);
  });
</script>

<Modal
  bind:open={() => picking !== null, (open) => !open && (flow.picking = null)}
  eyebrow="Weekly timing · you own it"
  title={picking ? `Hand ${whenWords(picking.timing)} ${picking.timing.bosses.map((b) => b.name).join(' + ')} to…` : ''}
  narrow
  className={phone ? 'own-sheet' : ''}
  returnFocus={() => rowControl(about)}
  data-fid="timings-own-picker"
>
  {#if picking}
    <p class="own-dialog__lead">Pick who owns it next. Ownership only moves between members of the party.</p>
    <fieldset class="own-pick">
      <legend class="vh">New owner</legend>
      {#each picking.timing.party.filter((m) => m.id !== picking.timing.owner.id) as person (person.id)}
        {@const ask = picking.timing.requests.find((r) => r.status === 'open' && r.requester.id === person.id)}
        <label class="own-pick__row" class:own-pick__row--on={person.id === picking.to}>
          <input class="own-pick__input" type="radio" name="own-to" value={person.id} checked={person.id === picking.to} onchange={() => flow.pick(picking.timing, person.id)} />
          <span class="own-pick__radio" aria-hidden="true">{#if person.id === picking.to}<Icon name="check" />{/if}</span>
          <Avatar src={null} name={person.name} class="own-avatar" />
          <span class="own-pick__name">{person.name}</span>
          {#if ask && list.data}<span class="status-chip status-chip--warn">asked to own · <span class="mono">{expiresWords(ask, list.data.generated_at)}</span></span>{/if}
        </label>
      {/each}
    </fieldset>
    {#if to}
      {@const closing = picking.timing.requests.filter((r) => r.status === 'open').map((r) => `${r.requester.name}'s`)}
      <p class="infobox own-dialog__box">
        <OwnIcon name="crown" /><span
          ><b>You → {to.name}.</b> Discord posts “👑 {to.name} now owns weekly timing {tokenWords(picking.timing)} · {whenWords(picking.timing)}”.{closing.length
            ? ` ${closing.join(' and ')} ${closing.length === 1 ? 'ask closes' : 'asks close'}: its owner changed.`
            : ''}</span
        >
      </p>
    {/if}
    {#if phone}<p class="field__hint">Needs {freshWords}.</p>{/if}
  {/if}
  {#snippet footer(close)}
    {#if !phone}<span class="field__hint own-dialog__hint">Needs {freshWords}</span>{/if}
    <button type="button" class="btn" onclick={close}>Cancel</button>
    <button
      type="button"
      class="btn btn--primary"
      disabled={!to || list.busy !== ''}
      aria-busy={list.busy !== ''}
      onclick={() => picking && to && void flow.run({ kind: 'hand', timing: picking.timing, to })}><OwnIcon name="crown" />Hand to {to?.name ?? '…'}</button
    >
  {/snippet}
</Modal>

<Modal
  bind:open={() => confirming !== null, (open) => !open && (flow.confirming = null)}
  eyebrow="Fresh sign-in"
  title="Confirm it's you"
  narrow
  className={phone ? 'own-sheet' : ''}
  returnFocus={() => rowControl(about)}
  data-fid="confirm"
>
  {#if confirming}
    {@const what = `${tokenWords(confirming.timing)}, ${whenWords(confirming.timing)}`}
    <p class="own-dialog__lead">
      {confirming.kind === 'hand' ? 'Handing over a weekly timing' : 'Accepting an ask to own'} needs {freshWords}.{fresh ? ' Yours was at ' : ''}{#if fresh}<span
          class="mono">{fresh.at}</span
        >.{/if}
    </p>
    <p class="infobox own-dialog__box">
      <OwnIcon name="save" /><span>
        {#if confirming.kind === 'hand'}
          <b>Not saved yet:</b> {confirming.to.name} as owner of {what}. After signing in you're back on Weekly timings with {confirming.to.name} picked; press Hand to {confirming.to.name}
          once more.
        {:else if confirming.kind === 'accept'}
          <b>Not saved yet:</b> {confirming.request.requester.name} as owner of {what}. After signing in you're back on Weekly timings; press Accept once more.
        {/if}
      </span>
    </p>
    <p class="field__hint">
      {confirming.kind === 'hand' ? 'Accepting an ask to own' : 'Handing over a timing'} needs the same sign-in. Asking, declining and withdrawing don't.
    </p>
  {/if}
  {#snippet footer(close)}
    <button type="button" class="btn" onclick={close}>Not now</button>
    {#if confirming}
      <a class="btn btn--primary" href={discordStart(returnPath(confirming))}><Icon name="log-in" />Sign in again</a>
    {/if}
  {/snippet}
</Modal>
