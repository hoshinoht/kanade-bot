import { reducedMotion } from '../motion/easing';
import { EXIT_FALLBACK_MS } from '../motion/presence.svelte';

export type ToastTone = 'info' | 'ok' | 'error';

export interface ToastAction {
  label: string;
  run: () => void;
}

export interface Toast {
  id: number;
  message: string;
  tone: ToastTone;
  action?: ToastAction;
  /** null keeps the toast until dismissed. */
  timeoutMs: number | null;
}

/** Success/info toasts hide after this; one offering an action (Undo) gets longer. */
export const TOAST_MS = 6000;
export const TOAST_ACTION_MS = 10_000;
/** The stack never holds more than this, so it never covers the window. */
export const TOAST_MAX = 2;

/** Errors stay until dismissed; the rest hide after 6 s, or 10 s with an action. */
function defaultTimeout(tone: ToastTone, action?: ToastAction): number | null {
  if (tone === 'error') return null;
  return action ? TOAST_ACTION_MS : TOAST_MS;
}

export class Toaster {
  /** Live toasts, newest last (the region shows them newest on top). */
  items = $state<Toast[]>([]);
  /** Dismissed toasts still playing their exit; shown inert and hidden from assistive tech. */
  leaving = $state<Toast[]>([]);
  #next = 1;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- exit timers, never rendered
  #timers = new Map<number, ReturnType<typeof setTimeout>>();

  /** Live and leaving toasts, oldest first. */
  get shown(): Toast[] {
    if (!this.leaving.length) return this.items;
    return [...this.leaving, ...this.items].sort((a, b) => a.id - b.id);
  }

  show(toast: Omit<Toast, 'id' | 'timeoutMs' | 'tone'> & Partial<Pick<Toast, 'timeoutMs' | 'tone'>>): number {
    const id = this.#next++;
    const tone = toast.tone ?? 'info';
    const timeoutMs = toast.timeoutMs === undefined ? defaultTimeout(tone, toast.action) : toast.timeoutMs;
    const next: Toast = { ...toast, tone, timeoutMs, id };
    // Over the cap, a timed toast goes first (oldest first): errors and
    // "Reload" prompts are only pushed out when every older toast is one.
    const older = [...this.items];
    while (older.length >= TOAST_MAX) {
      const victim = older.find((t) => t.timeoutMs !== null) ?? older[0]!;
      older.splice(older.indexOf(victim), 1);
      this.#leave(victim);
    }
    this.items = [...older, next];
    return id;
  }

  dismiss(id: number): void {
    const toast = this.items.find((t) => t.id === id);
    if (!toast) return;
    this.items = this.items.filter((t) => t.id !== id);
    this.#leave(toast);
  }

  /** The exit animation ended (or its fallback fired): drop the node. */
  gone(id: number): void {
    clearTimeout(this.#timers.get(id));
    this.#timers.delete(id);
    if (this.leaving.some((t) => t.id === id)) this.leaving = this.leaving.filter((t) => t.id !== id);
  }

  #leave(toast: Toast): void {
    if (reducedMotion()) return;
    this.leaving = [...this.leaving, toast];
    this.#timers.set(toast.id, setTimeout(() => this.gone(toast.id), EXIT_FALLBACK_MS));
  }
}
