<!--
  One open ownership request: the weekly timing, who asks and who owns it
  now, how long is left, and Accept / Decline. Staff decide for anyone;
  accepting pins the requester as the timing's owner, declining leaves it.
-->
<script lang="ts">
  import type { OwnershipRequest } from '@kanade/api-types';
  import { BossTag, DecisionCard, Icon, PendingLabel, Portrait, ThreadPanel, WavyProgress } from '@kanade/ui';
  import { localAt } from '../history/describe';
  import Name from '../names/Name.svelte';
  import { EXPIRY_WARN } from './expiry';
  import { expiresIn, minutesLeft } from './ownership';

  let {
    r,
    now = '',
    timeZone,
    busy = '',
    error = '',
    ondecide,
  }: {
    r: OwnershipRequest;
    /** The server's clock (`Week.generated_at`) for the expiry bar; empty hides it. */
    now?: string;
    timeZone: string;
    /** The decision in flight, if any. */
    busy?: '' | 'accept' | 'decline';
    error?: string;
    ondecide: (accept: boolean) => void;
  } = $props();
  const uid = $props.id();
  // A request lives 24 h from when it was asked.
  const span = $derived((Date.parse(r.expires_at) - Date.parse(r.created_at)) / 60_000);
  const left = $derived(now ? minutesLeft(r, now) : null);
  const warn = $derived(left !== null && left <= span * EXPIRY_WARN);
</script>

<article class="proposal" aria-labelledby="{uid}-title">
  <div class="proposal__main" data-fid="inbox-detail">
    <header class="proposal__head" data-fid="inbox-head">
      {#if r.bosses[0]}<span class="proposal__art" aria-hidden="true"><Portrait boss={r.bosses[0]} size="md" /></span>{/if}
      <div class="proposal__headtext">
        <h2 class="proposal__title" id="{uid}-title">
          Ownership — {#each r.bosses as boss (boss.token)}<BossTag {boss} />{/each}
        </h2>
        <p class="proposal__meta">
          <span class="chip proposal__source">Ownership request</span>
          <span class="proposal__fact proposal__fact--aside mono">#{r.short_id}</span>
          <span class="proposal__fact">asked <time datetime={r.created_at}>{localAt(r.created_at, timeZone)}</time></span>
          {#if r.channel}<span class="proposal__fact">{r.channel}</span>{/if}
        </p>
      </div>
    </header>

    <ThreadPanel label="Ownership request">
      <div class="proposal__bubble">
        <p class="cap">Asked by <Name kind="member" id={r.requester.id} name={r.requester.name} /> <span class="vh">, a party member.</span></p>
      </div>
      <div class="proposal__would">
        <h3 class="cap proposal__cap">Would change<span class="proposal__capfield">· Owner</span></h3>
        <ul class="proposal__changes">
          <li>
            <span class="proposal__people">
              <del class="proposal__person proposal__person--out"><Name kind="member" id={r.owner.id} name={r.owner.name} plain /><span class="vh"> (owns it now)</span></del>
              <span aria-hidden="true">→</span>
              <ins class="proposal__person proposal__person--in"><Name kind="member" id={r.requester.id} name={r.requester.name} plain /><span class="vh"> (would own it)</span></ins>
            </span>
          </li>
        </ul>
        <p class="note">Weekly timing <a class="mono" href="/fixed?open={encodeURIComponent(r.fixed_id)}">#{r.fixed_short_id}</a> · <span class="mono">{r.weekday_name} {r.time}</span></p>
        {#if left !== null && left > 0}
          <div class="proposal__expiry" class:proposal__expiry--warn={warn} data-fid="inbox-expiry">
            <p class="proposal__expirytext">Expires <time class="mono" datetime={r.expires_at}>{localAt(r.expires_at, timeZone)}</time> · {expiresIn(r, now)}</p>
            <WavyProgress
              class="wavy--inline"
              value={left}
              max={span}
              wavy={false}
              tone={warn ? 'warn' : 'accent'}
              label="Time left to decide"
              text="{expiresIn(r, now)}, expires {localAt(r.expires_at, timeZone)}"
            />
          </div>
        {/if}
      </div>
    </ThreadPanel>
  </div>

  <DecisionCard label="Decide this request">
    <button class="btn btn--primary btn--key decision__approve" data-fid="decision-approve" type="button" disabled={Boolean(busy)} onclick={() => ondecide(true)}
      ><Icon name="check" /><PendingLabel pending={busy === 'accept'} label="Accepting…">Accept</PendingLabel></button
    >
    <span class="decision__spacer" aria-hidden="true"></span>
    <button class="btn btn--danger decision__reject" data-fid="decision-reject" type="button" disabled={Boolean(busy)} onclick={() => ondecide(false)}
      ><PendingLabel pending={busy === 'decline'} label="Declining…">Decline</PendingLabel></button
    >
    <p class="decision__foot" data-fid="decision-foot">Accept makes the member the timing's owner; Decline leaves it as it is. Discord's request post is updated either way.</p>
    <p class="field__error" id="{uid}-err" role="alert">{error}</p>
  </DecisionCard>
</article>
