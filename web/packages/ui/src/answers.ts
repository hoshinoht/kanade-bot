/** A party's answers counted: the segments of `AnswerBar`. */
export interface AnswerCounts {
  yes: number;
  maybe: number;
  waiting: number;
  no: number;
  total: number;
}

export function answerCounts(participants: readonly { answer: string }[]): AnswerCounts {
  const counts: AnswerCounts = { yes: 0, maybe: 0, waiting: 0, no: 0, total: participants.length };
  for (const p of participants) {
    if (p.answer === 'yes' || p.answer === 'maybe' || p.answer === 'no') counts[p.answer] += 1;
    else counts.waiting += 1;
  }
  return counts;
}

/** "3 on, 1 maybe, 2 waiting, 1 out of 7": every count said, the empty ones left out. */
export function answerWords(c: AnswerCounts): string {
  const parts = [
    c.yes ? `${c.yes} on` : '',
    c.maybe ? `${c.maybe} maybe` : '',
    c.waiting ? `${c.waiting} waiting` : '',
    c.no ? `${c.no} out` : '',
  ].filter(Boolean);
  return `${parts.join(', ') || 'no answers'} of ${c.total}`;
}
