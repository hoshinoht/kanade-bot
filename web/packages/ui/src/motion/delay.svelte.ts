/** Waits shorter than this show nothing (spec "Motion and loading"): a flash of a spinner reads slower than none. */
export const LOADING_DELAY_MS = 200;

/** Turns `due` true once `ms` have passed since `start()`, unless stopped first. */
export class Delay {
  due = $state(false);
  #ms: number;
  #timer: ReturnType<typeof setTimeout> | null = null;

  constructor(ms = LOADING_DELAY_MS) {
    this.#ms = ms;
  }

  start(): void {
    this.stop();
    this.#timer = setTimeout(() => {
      this.#timer = null;
      this.due = true;
    }, this.#ms);
  }

  stop(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.due = false;
  }
}
