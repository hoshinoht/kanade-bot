import { untrack } from 'svelte';
import { reducedMotion } from './easing';

/** Longest exit we wait for when no animationend arrives (hidden tab, animation none, unmounted node). */
export const EXIT_FALLBACK_MS = 300;

/**
 * One value that outlives its removal by its exit animation: render `shown`,
 * mark the node `is-leaving` while `leaving`, and call `done()` from
 * `animationend`. Setting a new value while leaving cancels the exit.
 * Reduced motion removes at once.
 */
export class Presence<T> {
  shown = $state<T | null>(null);
  leaving = $state(false);
  #timer: ReturnType<typeof setTimeout> | null = null;

  constructor(initial: T | null = null) {
    this.shown = initial;
  }

  /** Safe to call from an effect: it never subscribes the caller to its own state. */
  set(next: T | null): void {
    untrack(() => this.#set(next));
  }

  #set(next: T | null): void {
    if (next !== null) {
      this.#clear();
      this.leaving = false;
      this.shown = next;
      return;
    }
    if (this.shown === null || this.leaving) return;
    if (reducedMotion()) {
      this.shown = null;
      return;
    }
    this.leaving = true;
    this.#timer = setTimeout(() => this.done(), EXIT_FALLBACK_MS);
  }

  /** The exit finished: drop the value. Ignores events that are not ours (e.g. a child's animationend). */
  done(event?: Event): void {
    if (event && event.target !== event.currentTarget) return;
    if (!this.leaving) return;
    this.#clear();
    this.leaving = false;
    this.shown = null;
  }

  #clear(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
  }
}
