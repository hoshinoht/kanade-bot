<!--
  One row's lane cell (B_HistoryGraph): the History line, each draft's lane
  through the row and the glyphs, drawn by shape and line style (classes in
  _history-graph.scss); colour only repeats them. aria-hidden: the row's
  words say everything drawn here.
-->
<script lang="ts">
  import type { Lane, Line, Node } from './graph';

  let {
    lanes,
    line,
    node,
    width,
    x,
    selected = false,
    selectedDraft = '',
    bad,
  }: {
    lanes: Lane[];
    line: Line;
    node: Node;
    width: number;
    /** A lane's centre (0 is the History line). */
    x: (lane: number) => number;
    /** The row's change is the one open in the pane. */
    selected?: boolean;
    /** The draft open in the pane: its lane is drawn heavier. */
    selectedDraft?: string;
    /** Whether a draft has known conflicts (its tip fills with "!"). */
    bad: (id: string) => boolean;
  } = $props();

  // A line from a row's top starts 1.5 px early, so sub-pixel row heights never leave a gap.
  const LINE: Record<Line, [string, string] | null> = { none: null, full: ['-1.5', '100%'], top: ['-1.5', '50%'], bottom: ['50%', '100%'], dash: ['-1.5', '100%'] };
  const x0 = $derived(x(0));
  const span = $derived(LINE[line]);
  const stroke = (lane: Lane) => `glane__d glane__d--${lane.draft.status}${lane.draft.id === selectedDraft ? ' glane__d--sel' : ''}`;
</script>

<!-- data-events lists what the lanes draw here (the text-equals-graph check reads it). -->
<td class="glane" aria-hidden="true" data-events={lanes.filter((l) => l.seg !== 'pass').map((l) => `${l.seg}:${l.draft.title}`).join('|') || undefined}>
  <svg {width} height="100%" focusable="false">
    {#if span}<line class="glane__l0" class:glane__l0--dash={line === 'dash'} x1={x0} x2={x0} y1={span[0]} y2={span[1]}></line>{/if}
    <!-- Passing lines first, curves over them (haloed), glyphs last. -->
    {#each lanes as l (l.draft.id)}
      {#if l.seg === 'pass' || l.seg === 'stub'}<line class={stroke(l)} x1={x(l.lane)} x2={x(l.lane)} y1="-1.5" y2={l.seg === 'stub' ? '85%' : '100%'}></line>{/if}
    {/each}
    {#each lanes as l (l.draft.id)}
      {@const xi = x(l.lane)}
      {#if l.seg === 'base'}
        <line class={stroke(l)} x1={xi} x2={xi} y1="-1.5" y2="1"></line>
        <svg x="0" y="0" {width} height="50%" viewBox="0 0 {width} 100" preserveAspectRatio="none" overflow="visible">
          <path class="glane__halo" d="M{x0} 100 C{x0} 45 {xi} 55 {xi} 0"></path>
          <path class={stroke(l)} d="M{x0} 100 C{x0} 45 {xi} 55 {xi} 0"></path>
        </svg>
      {:else if l.seg === 'merge'}
        <svg x="0" y="50%" {width} height="50%" viewBox="0 0 {width} 100" preserveAspectRatio="none" overflow="visible">
          <path class="glane__halo" d="M{xi} 100 C{xi} 45 {x0} 55 {x0} 0"></path>
          <path class={stroke(l)} d="M{xi} 100 C{xi} 45 {x0} 55 {x0} 0"></path>
        </svg>
      {:else if l.seg === 'tip' || l.seg === 'end' || l.seg === 'endIn'}
        <line class={stroke(l)} x1={xi} x2={xi} y1="50%" y2="100%"></line>
      {/if}
    {/each}
    {#each lanes as l (l.draft.id)}
      {@const xi = x(l.lane)}
      {#if l.seg === 'tip'}
        <svg x={xi} y="50%" overflow="visible">
          {#if bad(l.draft.id)}
            <rect class="glane__tip glane__tip--bad" x="-6.5" y="-6.5" width="13" height="13" rx="3.5"></rect>
            <path class="glane__bang" d="M0 -3.4V0.8M0 3.3v.2"></path>
          {:else}
            <rect class="glane__tip" x="-6.5" y="-6.5" width="13" height="13" rx="3.5"></rect>
          {/if}
          {#if l.draft.id === selectedDraft}<rect class="glane__ring" x="-10.5" y="-10.5" width="21" height="21" rx="6"></rect>{/if}
        </svg>
      {:else if l.seg === 'end' && l.draft.status === 'expired'}
        <svg x={xi} y="50%" overflow="visible"><path class="glane__expired" d="M-6.5 0H6.5"></path></svg>
      {:else if l.seg === 'end'}
        <svg x={xi} y="50%" overflow="visible"><circle class="glane__halo-dot" r="8"></circle><path class="glane__cross" d="M-4.5 -4.5L4.5 4.5M4.5 -4.5L-4.5 4.5"></path></svg>
      {:else if l.seg === 'stub'}
        <svg x={xi} y="100%" overflow="visible"><path class="glane__arrow" d="M-4.5 -10L0 -4.5L4.5 -10"></path></svg>
      {/if}
    {/each}
    {#if node === 'rec' && selected}
      <svg x={x0} y="50%" overflow="visible"><circle class="glane__ring" r="9.5"></circle><circle class="glane__sel" r="5.5"></circle></svg>
    {:else if node === 'rec'}
      <svg x={x0} y="50%" overflow="visible"><circle class="glane__rec" r="5"></circle></svg>
    {:else if node === 'head'}
      <svg x={x0} y="50%" overflow="visible"><circle class="glane__head" r="6"></circle>{#if selected}<circle class="glane__ring" r="10.5"></circle>{/if}</svg>
    {:else if node === 'merge'}
      <svg x={x0} y="50%" overflow="visible"><circle class="glane__merge" r="7"></circle><circle class="glane__merge-dot" r="2.5"></circle>{#if selected}<circle class="glane__ring" r="11"></circle>{/if}</svg>
    {:else if node === 'fold'}
      <svg x={x0} y="50%" overflow="visible"><rect class="glane__fold" x="-5" y="-10" width="10" height="20" rx="5"></rect></svg>
    {/if}
  </svg>
</td>
