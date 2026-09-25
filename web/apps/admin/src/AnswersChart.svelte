<script lang="ts">
  import uPlot from 'uplot';
  import 'uplot/dist/uPlot.min.css';
  import type { Stats, Week } from '@kanade/api-types';
  import { dayLabel } from '@kanade/ui';

  let { stats, week }: { stats: Stats; week: Week } = $props();

  let host: HTMLDivElement;
  const uid = $props.id();
  const totals = $derived(
    stats.per_day.reduce((acc, d) => ({ answered: acc.answered + d.answered, waiting: acc.waiting + d.waiting }), { answered: 0, waiting: 0 }),
  );

  function token(name: string): string {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || '#888';
  }

  // uPlot draws on canvas and sizes itself through CSSOM (el.style.*), which
  // CSP allows; it injects no style element and no markup strings.
  $effect(() => {
    const data: uPlot.AlignedData = [
      stats.per_day.map((d) => d.day),
      stats.per_day.map((d) => d.answered),
      stats.per_day.map((d) => d.waiting),
    ];
    let chart: uPlot | null = null;

    const build = () => {
      chart?.destroy();
      const ink = token('--dim');
      const grid = token('--line-soft');
      const font = `12px ${token('--mono')}`;
      const bars = (align: -1 | 1) => uPlot.paths.bars!({ size: [0.36, 48], align });
      chart = new uPlot(
        {
          width: Math.max(280, host.clientWidth),
          height: 220,
          legend: { show: true },
          cursor: { drag: { x: false, y: false } },
          scales: { x: { time: false, range: [-0.6, 6.6] }, y: { range: (_u, _min, max) => [0, Math.max(4, max + 1)] } },
          axes: [
            { stroke: ink, font, grid: { show: false }, ticks: { stroke: grid }, values: (_u, splits) => splits.map((s) => (Number.isInteger(s) ? dayLabel(week, s) : '')), splits: () => [0, 1, 2, 3, 4, 5, 6] },
            { stroke: ink, font, grid: { stroke: grid }, ticks: { stroke: grid } },
          ],
          series: [
            { label: 'Day', value: (_u, v) => (v == null ? '–' : dayLabel(week, v)) },
            { label: 'Answered', stroke: token('--ok'), fill: token('--ok'), width: 2, paths: bars(-1), points: { show: false } },
            // Waiting is hollow and dashed, so the two series differ without colour.
            { label: 'Waiting', stroke: token('--warn'), fill: 'transparent', width: 2, dash: [4, 3], paths: bars(1), points: { show: false } },
          ],
        },
        data,
        host,
      );
    };

    build();
    const resize = new ResizeObserver(() => chart?.setSize({ width: Math.max(280, host.clientWidth), height: 220 }));
    resize.observe(host);
    const themed = new MutationObserver(build);
    themed.observe(document.documentElement, { attributeFilter: ['data-colorway', 'data-theme'] });
    const scheme = matchMedia('(prefers-color-scheme: dark)');
    scheme.addEventListener('change', build);
    return () => {
      resize.disconnect();
      themed.disconnect();
      scheme.removeEventListener('change', build);
      chart?.destroy();
    };
  });
</script>

<h2 class="card__title chart__title" id="{uid}-title">Answers by day</h2>
<p class="chart__sum">{totals.answered} answered, {totals.waiting} still waiting.</p>
<figure class="chart" aria-labelledby="{uid}-title">
  <div class="chart__canvas" bind:this={host} role="img" aria-label="Bar chart of answered and waiting replies per day; the table below has the same figures."></div>
</figure>
<div class="table-wrap">
  <table>
    <caption>Answers by day</caption>
    <thead>
      <tr><th scope="col">Day</th><th scope="col" class="num">Answered</th><th scope="col" class="num">Waiting</th></tr>
    </thead>
    <tbody>
      {#each stats.per_day as d (d.day)}
        <tr><th scope="row">{dayLabel(week, d.day)}</th><td class="num">{d.answered}</td><td class="num">{d.waiting}</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .chart__title {
    margin: 0.3rem 0 0.1rem;
  }

  .chart__sum {
    margin: 0 0 0.5rem;
    color: var(--dim);
    font-size: var(--fs-small);
  }

  .chart {
    margin: 0 0 0.8rem;
  }

  .chart__canvas {
    min-height: 220px;
    color: var(--ink);
  }

  .chart__canvas :global(.u-legend) {
    font-family: var(--mono);
    font-size: var(--fs-mini);
    color: var(--dim);
  }
</style>
