<!--
  A MapleScouter lookup in progress (a job of up to 2 minutes): who is looked
  up, the time so far, a bar, and that the page can be closed.
-->
<script lang="ts">
  import { WavyProgress } from '@kanade/ui';
  import { clock } from './format';

  let {
    done,
    total,
    who,
    elapsed,
    ...rest
  }: { done: number; total: number; who: string; elapsed: number; [data: `data-${string}`]: string | undefined } = $props();
  // One character has no steps to count: the bar follows the time, out of the 2 minutes.
  const value = $derived(total > 1 ? done : Math.min(elapsed, 120));
  const max = $derived(total > 1 ? total : 120);
</script>

<!-- Only who is looked up is announced; the ticking time is not. -->
<div class="estimate__progress" {...rest}>
  <span class="estimate__progress-line"><span role="status">{who}</span><span class="mono">{clock(elapsed)} of up to 2:00</span></span>
  <WavyProgress value={Math.max(value, max * 0.08)} {max} label="Lookup progress" text={total > 1 ? `${done} of ${total}` : clock(elapsed)} />
  <span class="chardata__sub">You can close this. The result stays for 24 h and shows here when you come back.</span>
</div>
