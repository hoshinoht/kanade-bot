// This browser's "Reduce motion" switch (Account › This browser). On, the app
// moves as it does under the device's `prefers-reduced-motion: reduce`; off, it
// follows the device, so a device that asks to reduce is never overridden. The
// choice is reflected as `<html data-motion="reduce">`, which theme-boot.js sets
// before first paint, the CSS reads (styles/_motion-query.scss) and
// `reducedMotion()` checks. It stays in this browser and is never sent anywhere.
import { prefersReducedMotion } from 'svelte/motion';

export const MOTION_KEY = 'kanade.motion';

function applied(): boolean {
  return typeof document !== 'undefined' && document.documentElement.dataset.motion === 'reduce';
}

class MotionPreference {
  /** The switch itself; theme-boot.js has already applied a stored choice. */
  reduce = $state(applied());

  /** Reactive: the device or this browser asks for reduced motion. */
  get reduced(): boolean {
    return this.reduce || prefersReducedMotion.current;
  }

  set(reduce: boolean): void {
    this.reduce = reduce;
    if (typeof document !== 'undefined') {
      if (reduce) document.documentElement.dataset.motion = 'reduce';
      else delete document.documentElement.dataset.motion;
    }
    try {
      if (reduce) localStorage.setItem(MOTION_KEY, 'reduce');
      else localStorage.removeItem(MOTION_KEY);
    } catch {
      /* storage blocked: the choice lasts for this page only */
    }
  }
}

export const motionPreference = new MotionPreference();
