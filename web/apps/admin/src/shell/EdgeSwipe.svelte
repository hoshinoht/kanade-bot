<!--
  The phone's left-edge swipe that opens the navigation drawer (M3E spec
  "Phone": "Opens from the menu button or a left-edge swipe"). A thin strip
  over the frame's left margin with `touch-action: pan-y`, so a sideways
  stroke stays a pointer gesture (the browser does not take it over and
  cancel it) while vertical scrolling passes through. The menu button is the
  equivalent control, so the strip is hidden from assistive technology.
-->
<script lang="ts">
  let { onopen }: { onopen: () => void } = $props();

  let start: { id: number; x: number; y: number } | null = null;

  function down(event: PointerEvent) {
    if (event.pointerType === 'mouse') return;
    start = { id: event.pointerId, x: event.clientX, y: event.clientY };
    // Keep receiving the stroke once it leaves the strip.
    try {
      (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    } catch {
      /* not an active pointer (a synthetic event): moves still reach the strip */
    }
  }

  function move(event: PointerEvent) {
    if (!start || event.pointerId !== start.id) return;
    const dx = event.clientX - start.x;
    // A mostly horizontal stroke inward of about a thumb's width.
    if (dx > 48 && dx > 2 * Math.abs(event.clientY - start.y)) {
      start = null;
      onopen();
    }
  }

  const end = () => (start = null);
</script>

<div class="edgeswipe" aria-hidden="true" onpointerdown={down} onpointermove={move} onpointerup={end} onpointercancel={end}></div>
