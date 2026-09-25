<!--
  Experiment B: determinate progress whose filled part is a gentle sine wave
  drifting forward; the rest is a flat track after a small gap. Paths are
  recomputed per frame (attributes, not styles, so CSP-safe) so both ends keep
  round caps; the wave flattens at 100% and under reduced motion.
-->
<script lang="ts">
  let { value, max, label, text = '' }: { value: number; max: number; label: string; text?: string } = $props();

  const STROKE = 4;
  const AMP = 3;
  const WAVELENGTH = 32;
  /** Seconds per wavelength of drift: slow enough to read as calm. */
  const DRIFT_S = 2;
  const GAP = 6;
  const HEIGHT = STROKE + 2 * AMP + 2;
  const MID = HEIGHT / 2;

  let width = $state(0);
  let wave = $state('');
  let track = $state('');
  const target = $derived(max > 0 ? Math.min(1, Math.max(0, value / max)) : 0);

  let shown = 0;
  let amp = 0;
  let phase = 0;

  function draw(w: number) {
    const usable = Math.max(0, w - STROKE);
    const start = STROKE / 2;
    const end = start + shown * usable;
    let d = '';
    if (shown > 0) {
      for (let x = start; ; x = Math.min(end, x + 2)) {
        const y = MID + amp * Math.sin((2 * Math.PI * (x - phase)) / WAVELENGTH);
        d += `${d ? 'L' : 'M'}${x.toFixed(1)} ${y.toFixed(2)}`;
        if (x >= end) break;
      }
    }
    wave = d;
    const from = shown > 0 ? end + STROKE + GAP : start;
    const to = w - STROKE / 2;
    track = from < to ? `M${from.toFixed(1)} ${MID}L${to.toFixed(1)} ${MID}` : '';
  }

  $effect(() => {
    const goal = target;
    const w = width;
    if (!w) return;
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) {
      shown = goal;
      amp = 0;
      draw(w);
      return;
    }
    let frame = 0;
    let last = performance.now();
    const step = (now: number) => {
      const dt = Math.min(0.064, (now - last) / 1000);
      last = now;
      // Critically damped approach: progress never overshoots its value.
      shown += (goal - shown) * (1 - Math.exp(-dt * 9));
      if (Math.abs(goal - shown) < 0.0005) shown = goal;
      // Short fills and a finished bar lose the wave; a long fill carries it fully.
      const want = goal >= 1 ? 0 : AMP * Math.min(1, (shown * w) / WAVELENGTH);
      amp += (want - amp) * (1 - Math.exp(-dt * 6));
      phase = (phase + (dt * WAVELENGTH) / DRIFT_S) % WAVELENGTH;
      draw(w);
      if (goal >= 1 && shown === goal && amp < 0.02) {
        amp = 0;
        draw(w);
        return;
      }
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  });
</script>

<div
  class="xp-wavy"
  role="progressbar"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={max}
  aria-valuenow={value}
  aria-valuetext={text || undefined}
  bind:clientWidth={width}
>
  <svg class="xp-wavy__svg" height={HEIGHT} aria-hidden="true" focusable="false">
    <path class="xp-wavy__track" d={track} />
    <path class="xp-wavy__wave" d={wave} />
  </svg>
</div>
