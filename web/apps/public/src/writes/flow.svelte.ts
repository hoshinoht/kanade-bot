// What a press on Answer or Move sets going (boards Week-RunMine,
// ConfirmAnswer, PhoneRun, MyRuns, Move): the write, then a toast on a
// refusal or a stale week (saying what changed), or "Confirm it's you" when
// the server wants a fresh sign-in (nothing saved; the choice comes back with
// the sign-in). A move's own success is the caller's to show.
import type { MemberMoveResult, MemberRun, MemberWeek } from '@kanade/api-types';
import { runTitle, type Slot, type Toaster } from '@kanade/ui';
import type { WeekKey } from '../weeks.svelte';
import type { RunWrites } from './runWrites.svelte';
import { answerReturn, choiceLabel, instantOf, moveReturn, slotWords, type Choice } from './runs';

/** The write waiting for a fresh sign-in, with where the sign-in comes back to. */
export type Pending =
  | { kind: 'answer'; run: MemberRun; week: MemberWeek; answer: Choice; next: string }
  | { kind: 'move'; run: MemberRun; week: MemberWeek; slot: Slot; next: string };

export class RunFlow {
  confirming = $state<Pending | null>(null);
  #writes: RunWrites;
  #toaster: Toaster;

  constructor(writes: RunWrites, toaster: Toaster) {
    this.#writes = writes;
    this.#toaster = toaster;
  }

  get busy(): string {
    return this.#writes.busy;
  }

  async answer(run: MemberRun, week: MemberWeek, which: WeekKey, memberId: string, answer: Choice): Promise<void> {
    const outcome = await this.#writes.answer(run.id, memberId, answer);
    if (!outcome || outcome.kind === 'done') return;
    if (outcome.kind === 'reauth') {
      this.confirming = { kind: 'answer', run, week, answer, next: answerReturn(run, which, answer) };
      return;
    }
    const why = outcome.kind === 'stale' ? `${outcome.words} Your answer (${choiceLabel(answer)}) was not saved.` : `Couldn't save your answer: ${outcome.words}`;
    this.#toaster.show({ message: why, tone: 'error' });
  }

  /** The move's result when it was made; the pick stays in the form otherwise. */
  async move(run: MemberRun, week: MemberWeek, memberId: string, slot: Slot): Promise<MemberMoveResult | null> {
    const outcome = await this.#writes.move(run.id, memberId, slot);
    if (!outcome) return null;
    if (outcome.kind === 'done') return outcome.result;
    if (outcome.kind === 'reauth') {
      this.confirming = { kind: 'move', run, week, slot, next: moveReturn(run, instantOf(slot, week)) };
      return null;
    }
    const why =
      outcome.kind === 'stale'
        ? `${outcome.words} Nothing was moved; your pick (${slotWords(week, slot)}) is still in the form.`
        : `Couldn't move ${runTitle(run)}: ${outcome.words}`;
    this.#toaster.show({ message: why, tone: 'error' });
    return null;
  }

  /** "Moved HCarling + HStar to Wed 30 21:30. The party is told on Discord." */
  moved(result: MemberMoveResult, week: Pick<MemberWeek, 'days'>): void {
    this.#toaster.show({ message: `Moved ${runTitle(result.run)} to ${slotWords(week, result.run)}. The party is told on Discord.`, tone: 'ok' });
  }
}
