// What a press on Weekly timings sets going (boards MyRuns-Timings-Owner
// frames 1–5): the Hand to… picker, the write, then a toast, a polite line or
// "Confirm it's you" when the owner change needs a fresh Discord sign-in.
import type { MemberTiming } from '@kanade/api-types';
import type { Toaster } from '@kanade/ui';
import { tick } from 'svelte';
import { doneWords, newOwner, refusalWords, type OwnWrite } from './ownership';
import type { MemberTimingsList } from './timings.svelte';

/** The address the fresh sign-in comes back to: Weekly timings, with the picker's choice when it was a hand-off. */
export function returnPath(write: OwnWrite): string {
  const base = '/mine?week=timings';
  return write.kind === 'hand' ? `${base}&hand=${encodeURIComponent(write.timing.id)}&to=${encodeURIComponent(write.to.id)}` : base;
}

/** The row's first control, where focus goes once its buttons changed. */
export function rowControl(timingId: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-timing="${CSS.escape(timingId)}"] button`);
}

export class OwnerFlow {
  /** The picker: which timing, and who is picked (a party member's id). */
  picking = $state<{ timing: MemberTiming; to: string } | null>(null);
  /** The owner change waiting for a fresh sign-in. */
  confirming = $state<OwnWrite | null>(null);
  /** The last change said politely (asks, declines, withdrawals show in the row). */
  said = $state('');
  #list: MemberTimingsList;
  #toaster: Toaster;

  constructor(list: MemberTimingsList, toaster: Toaster) {
    this.#list = list;
    this.#toaster = toaster;
  }

  pick(timing: MemberTiming, to: string): void {
    this.confirming = null;
    this.picking = { timing, to };
  }

  async run(write: OwnWrite): Promise<void> {
    const outcome = await this.#list.write(write);
    if (!outcome) return;
    if (!outcome.done && outcome.reauth) {
      this.picking = null;
      this.confirming = write;
      return;
    }
    const inPicker = this.picking !== null;
    this.picking = null;
    if (outcome.done) {
      if (newOwner(write)) this.#toaster.show({ message: doneWords(write), tone: 'ok' });
      else this.said = doneWords(write);
    } else {
      this.#toaster.show({ message: refusalWords(write, outcome.error), tone: 'error', action: { label: 'Reload', run: () => void this.#list.load() } });
    }
    // The pressed button may be gone (Ask became Withdraw); the picker returns focus itself.
    if (!inPicker) {
      await tick();
      rowControl(write.timing.id)?.focus({ preventScroll: true });
    }
  }
}
