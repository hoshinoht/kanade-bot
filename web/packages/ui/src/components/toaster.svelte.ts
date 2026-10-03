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

export class Toaster {
  /** Live toasts, newest last. */
  items = $state<Toast[]>([]);
  /** Dismissed toasts still playing their exit; shown inert and hidden from assistive tech. */
  leaving = $state<Toast[]>([]);
  #next = 1;
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- exit timers, never rendered
  #timers = new Map<number, ReturnType<typeof setTimeout>>();

  /** Live and leaving toasts in stack order, for the region to render. */
  get shown(): Toast[] {
    if (!this.leaving.length) return this.items;
    return [...this.leaving, ...this.items].sort((a, b) => a.id - b.id);
  }

  show(toast: Omit<Toast, 'id' | 'timeoutMs' | 'tone'> & Partial<Pick<Toast, 'timeoutMs' | 'tone'>>): number {
    const id = this.#next++;
    // Newest last; three at most so the stack never covers the window.
    const next: Toast = { tone: 'info', timeoutMs: 8000, ...toast, id };
    const all = [...this.items, next];
    all.slice(0, -3).forEach((old) => this.#leave(old));
    this.items = all.slice(-3);
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
