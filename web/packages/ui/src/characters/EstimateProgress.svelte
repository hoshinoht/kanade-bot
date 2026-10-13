<!--
  A MapleScouter lookup on its way: waiting in line ("Queued · 2 ahead", a
  flat bar and the time waited) or being looked up (who, the time so far, a
  bar of up to 2 minutes or of the characters done), and that the page can be
  closed. A member's line also says whose lookups share the line.
-->
<script lang="ts">
  import WavyProgress from '../components/WavyProgress.svelte';
  import { clock, queuedText } from './chardata';

  let {
    done,
    total,
    who,
    elapsed,
    queued = null,
    member = false,
    ...rest
  }: {
    done: number;
    total: number;
    who: string;
    elapsed: number;
    /** While waiting in line: the lookups ahead of it. */
    queued?: number | null;
    /** The portal: the line is shared with the admins' and other members' lookups. */
    member?: boolean;
    [data: `data-${string}`]: string | undefined;
  } = $props();
  // One character has no steps to count: the bar follows the time, out of the 2 minutes.
  const value = $derived(total > 1 ? done : Math.min(elapsed, 120));
  const max = $derived(total > 1 ? total : 120);
</script>

<!-- Only the place or who is looked up is announced; the ticking time is not. -->
<div class="estimate__progress" {...rest}>
  {#if queued !== null}
    <span class="estimate__progress-line"><span role="status">{queuedText(queued)}</span><span class="mono">waiting {clock(elapsed)}</span></span>
    <WavyProgress value={0} max={1} wavy={false} label="Place in line" text={queuedText(queued)} />
    <span class="chardata__sub"
      >Lookups run one at a time{member ? ', the admins’ and other members’ too' : ''}. You can close this: it runs when its turn comes, and the result stays for 24 h.</span
    >
  {:else}
    <span class="estimate__progress-line"><span role="status">{who}</span><span class="mono">{clock(elapsed)} of up to 2:00</span></span>
    <WavyProgress value={Math.max(value, max * 0.08)} {max} label="Lookup progress" text={total > 1 ? `${done} of ${total}` : clock(elapsed)} />
    <span class="chardata__sub">You can close this. The result stays for 24 h and shows here when you come back.</span>
  {/if}
</div>
