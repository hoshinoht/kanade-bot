<!--
  Determinate progress (m3e-rail-design-spec "Motion and loading"): the filled
  part is a gentle sine wave drifting forward while `wavy`, else a flat stroke;
  the rest is a flat track after a small gap. Paths are recomputed per frame
  (attributes, not styles, so CSP-safe) so both ends keep round caps; the wave
  flattens at 100% (unless `fullWave`: a full bar of live work keeps moving)
  and under reduced motion, and a flat bar stops drawing once its fill has
  settled. `ticks` mark fractions of the track (a countdown's
  T-1h and T-15m); `tone="warn"` takes the warning colour.
-->
<script lang="ts">
  let {
    value,
    max,
    label,
    text = '',
    wavy = true,
    fullWave = false,
    ticks = [],
    tone = 'accent',
    class: extra = '',
  }: {
    value: number;
    max: number;
    label: string;
    text?: string;
    /** The drifting wave; a flat bar keeps the same track, gap and caps. */
    wavy?: boolean;
    /** Keep waving at 100% (permits all held by calls in flight). */
    fullWave?: boolean;
    /** Marks along the track, each a fraction 0–1 of its length. */
    ticks?: number[];
    tone?: 'accent' | 'warn';
    class?: string;
  } = $props();

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
  const marks = $derived(
    width ? ticks.filter((t) => t > 0 && t < 1).map((t) => STROKE / 2 + t * Math.max(0, width - STROKE)) : [],
  );

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
    const moving = wavy;
    const holdFull = fullWave;
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
      // Short fills, a finished bar (unless held) and a flat bar lose the wave; a long fill carries it fully.
      const want = !moving || (goal >= 1 && !holdFull) ? 0 : AMP * Math.min(1, (shown * w) / WAVELENGTH);
      amp += (want - amp) * (1 - Math.exp(-dt * 6));
      phase = (phase + (dt * WAVELENGTH) / DRIFT_S) % WAVELENGTH;
      draw(w);
      if (want === 0 && shown === goal && amp < 0.02) {
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
  class="wavy wavy--{tone} {extra}"
  class:wavy--flat={!wavy}
  role="progressbar"
  aria-label={label}
  aria-valuemin={0}
  aria-valuemax={max}
  aria-valuenow={value}
  aria-valuetext={text || undefined}
  bind:clientWidth={width}
>
  <svg class="wavy__svg" height={HEIGHT} aria-hidden="true" focusable="false">
    <path class="wavy__track" d={track} />
    <path class="wavy__wave" d={wave} />
    {#each marks as x, i (i)}<line class="wavy__tick" x1={x.toFixed(1)} x2={x.toFixed(1)} y1={MID - 4} y2={MID + 4} />{/each}
  </svg>
</div>
