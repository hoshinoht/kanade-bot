import type { Attachment } from 'svelte/attachments';
import { ENTER_MS, STANDARD, reducedMotion } from './easing';

export type Direction = 'forward' | 'backward';

/** How far new content travels in: forward comes from the inline end, backward from the start. */
const TRAVEL_PX = 24;

/** Keyframes of the M3 forward/backward transition: a short slide plus a fade (transform and opacity only). */
export function enterFrames(direction: Direction): Keyframe[] {
  const from = direction === 'forward' ? TRAVEL_PX : -TRAVEL_PX;
  return [
    { opacity: 0, transform: `translateX(${from}px)` },
    { opacity: 1, transform: 'none' },
  ];
}

/**
 * Plays the enter transition on mount and again whenever `key` changes (the
 * attachment is recreated then), so a pane that stays mounted while its item
 * changes still reads as new content. A null/false key plays nothing.
 * Web Animations are CSP-safe; nothing is written to a style attribute.
 */
export function enter(key: unknown, direction: Direction = 'forward'): Attachment<HTMLElement> {
  return (node) => {
    if (key === null || key === undefined || key === false || reducedMotion() || typeof node.animate !== 'function') return;
    const animation = node.animate(enterFrames(direction), { duration: ENTER_MS, easing: STANDARD });
    return () => animation.cancel();
  };
}
