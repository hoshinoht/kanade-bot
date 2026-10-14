/**
 * Marks a horizontal scroller with `scroll-more-start` / `scroll-more-end`
 * while content hides past either edge, so CSS can fade that edge (classes,
 * not inline styles: the CSP bans `style` attributes).
 */
export function scrollEdges(node: HTMLElement): () => void {
  let frame = 0;
  const update = () => {
    frame = 0;
    const max = node.scrollWidth - node.clientWidth;
    // One pixel of slack: fractional widths leave sub-pixel overflow.
    node.classList.toggle('scroll-more-start', max > 1 && node.scrollLeft > 1);
    node.classList.toggle('scroll-more-end', max > 1 && node.scrollLeft < max - 1);
  };
  const schedule = () => {
    if (!frame) frame = requestAnimationFrame(update);
  };
  const resize = new ResizeObserver(schedule);
  const watch = () => {
    resize.disconnect();
    resize.observe(node);
    for (const child of node.children) resize.observe(child);
  };
  // Columns come and go with the week; re-observe whenever they change.
  const children = new MutationObserver(() => {
    watch();
    schedule();
  });
  watch();
  children.observe(node, { childList: true });
  node.addEventListener('scroll', schedule, { passive: true });
  update();
  return () => {
    if (frame) cancelAnimationFrame(frame);
    resize.disconnect();
    children.disconnect();
    node.removeEventListener('scroll', schedule);
    node.classList.remove('scroll-more-start', 'scroll-more-end');
  };
}
