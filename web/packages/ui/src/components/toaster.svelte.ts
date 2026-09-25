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
  items = $state<Toast[]>([]);
  #next = 1;

  show(toast: Omit<Toast, 'id' | 'timeoutMs' | 'tone'> & Partial<Pick<Toast, 'timeoutMs' | 'tone'>>): number {
    const id = this.#next++;
    // Newest last; three at most so the stack never covers the window.
    const next: Toast = { tone: 'info', timeoutMs: 8000, ...toast, id };
    this.items = [...this.items, next].slice(-3);
    return id;
  }

  dismiss(id: number): void {
    this.items = this.items.filter((t) => t.id !== id);
  }
}
