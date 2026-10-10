<!--
  My runs › Weekly timings (boards MyRuns-Timings-Owner, PhoneMyRuns-Timings-Owner,
  frames 1–2): every weekly timing the member is on, in boss-week order, with
  its owner crowned. The owner hands it to… a party member; anyone else asks
  to own it. Open asks on the member's own timing sit under it with Decline /
  Accept; the member's own ask shows as a chip with Withdraw. Always in,
  attendance marks and suggestions have no API yet and are not drawn.
-->
<script lang="ts">
  import type { MemberTiming } from '@kanade/api-types';
  import { Avatar, BossStack, Icon, LiveRegion, LoadError, LoadingState } from '@kanade/ui';
  import type { OwnerFlow } from './flow.svelte';
  import OwnIcon from './OwnIcon.svelte';
  import { expiresWords, incoming, pendingAsk, sortTimings, WEEKDAYS, whenWords, writePath } from './ownership';
  import type { MemberTimingsList } from './timings.svelte';

  let { list, flow, memberId, phone, startDay }: { list: MemberTimingsList; flow: OwnerFlow; memberId: string; phone: boolean; startDay: string | undefined } = $props();

  const timings = $derived(list.data ? sortTimings(list.data.timings, startDay) : []);
  const now = $derived(list.data?.generated_at ?? '');
</script>

{#snippet party(timing: MemberTiming)}
  <span class="account-row__sub timing__party">
    {#each [...timing.party].sort((a, b) => Number(b.id === memberId) - Number(a.id === memberId)) as person, index (person.id)}
      {@const name = person.id === memberId ? 'You' : person.name}
      {index > 0 ? ' · ' : ''}{#if person.id === timing.owner.id}<span class="owner" title="Owner"
          ><OwnIcon name="crown" />{name}<span class="vh">, owner</span></span
        >{:else}{name}{/if}
    {/each}
  </span>
{/snippet}

{#snippet action(timing: MemberTiming)}
  {@const to = timing.party.find((m) => m.id !== timing.owner.id)}
  {#if timing.you_own && to}
    <button
      type="button"
      class="btn timing__act"
      aria-haspopup="dialog"
      aria-label="Hand {whenWords(timing)} to another party member"
      aria-expanded={flow.picking?.timing.id === timing.id}
      disabled={list.busy !== ''}
      data-fid="timings-own-action"
      onclick={() => flow.pick(timing, to.id)}><OwnIcon name="arrow-right" />Hand to…</button
    >
  {:else if !timing.you_own}
    <button
      type="button"
      class="btn timing__act"
      aria-label="Ask to own {whenWords(timing)}"
      aria-busy={list.busy === writePath({ kind: 'ask', timing })}
      disabled={list.busy !== ''}
      data-fid="timings-own-action"
      onclick={() => void flow.run({ kind: 'ask', timing })}><OwnIcon name="crown" />Ask to own</button
    >
  {/if}
{/snippet}

{#snippet pending(timing: MemberTiming)}
  {@const ask = pendingAsk(timing)}
  {#if ask}
    <span class="status-chip status-chip--warn timing__chip" data-fid="timings-own-pending"
      ><Icon name="clock" />Asked {timing.owner.name} · expires in <span class="mono">{expiresWords(ask, now)}</span></span
    >
    {#if !phone}<span class="own-strip__text timing__dim">{timing.owner.name} can accept or decline here or in Discord.</span>{/if}
    <button
      type="button"
      class="btn btn--ghost timing__act"
      aria-label="Withdraw your ask to own {whenWords(timing)}"
      disabled={list.busy !== ''}
      onclick={() => void flow.run({ kind: 'withdraw', timing, request: ask })}>Withdraw</button
    >
  {/if}
{/snippet}

{#snippet requests(timing: MemberTiming)}
  {#each incoming(timing) as request (request.id)}
    <div class="own-strip own-strip--ask" class:own-strip--stack={phone} role="group" aria-label="{request.requester.name} asks to own this timing" data-fid="timings-own-request">
      <span class="own-strip__who">
        <Avatar src={null} name={request.requester.name} class="own-avatar" />
        <span class="own-strip__text"
          ><b>{request.requester.name}</b> asks to own this · <span class="nowrap">expires in <span class="mono">{expiresWords(request, now)}</span></span></span
        >
      </span>
      <span class="own-strip__acts">
        <button type="button" class="btn" disabled={list.busy !== ''} onclick={() => void flow.run({ kind: 'decline', timing, request })}
          >Decline<span class="vh"> {request.requester.name}'s ask</span></button
        >
        <button type="button" class="btn btn--primary" disabled={list.busy !== ''} onclick={() => void flow.run({ kind: 'accept', timing, request })}
          ><Icon name="check" />Accept<span class="vh"> {request.requester.name}'s ask</span></button
        >
      </span>
    </div>
  {/each}
{/snippet}

{#snippet when(timing: MemberTiming)}
  <span class="timing__when"><span class="cap">{WEEKDAYS[timing.weekday]}</span><span class="timing__time mono">{timing.time}</span></span>
{/snippet}

<div class="account-sec__head timings-head">
  <h2 class="account-sec__title" id="timings-title">Weekly timings you're in</h2>
  <span class="account-sec__note"><span class="owner"><OwnIcon name="crown" /><span class="vh">The crown</span></span> marks the owner, who answers asks to own the timing.</span>
</div>
{#if !list.data && list.error}
  <LoadError thing="your weekly timings" reason={list.error} onretry={() => void list.load()} level={3} />
{:else if !list.data}
  <LoadingState text="Loading your weekly timings…" />
{:else if timings.length === 0}
  <p class="note member-runs__none">You're not on any weekly timing.</p>
{:else}
  <ul class="account-grp timings" aria-labelledby="timings-title" data-fid="timings-list">
    {#each timings as timing (timing.id)}
      {@const mine = pendingAsk(timing)}
      <li class="account-row timing" data-timing={timing.id} data-fid="timings-row">
        {#if phone}
          {@render when(timing)}
          <BossStack bosses={timing.bosses} />
          {@render party(timing)}
          <div class="own-line" data-fid="timings-owner">
            {#if mine}{@render pending(timing)}{:else}<span class="account-row__sub">{timing.you_own ? 'You own this' : `${timing.owner.name} owns this`}</span
              >{@render action(timing)}{/if}
          </div>
        {:else}
          <div class="timing__top">
            {@render when(timing)}
            <div class="timing__mid">
              <BossStack bosses={timing.bosses} />
              <span class="own-line" data-fid="timings-owner">{@render party(timing)}{#if !mine}{@render action(timing)}{/if}</span>
            </div>
          </div>
          {#if mine}<div class="own-strip">{@render pending(timing)}</div>{/if}
        {/if}
        {@render requests(timing)}
      </li>
    {/each}
  </ul>
  <p class="infobox timings-note">
    <Icon name="info" /><span>Ownership only moves between members of the party; an ask to own expires after 24 hours.</span>
  </p>
{/if}
<LiveRegion message={flow.said} />
