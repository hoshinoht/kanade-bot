/**
 * Pure helpers for the Rewrites list-detail: the `?attempt=` selection in
 * the page's query string, a verdict with its rule or code in words, and
 * reported usage against the reservation it was checked against.
 */
import type { RewriteRow } from '@kanade/api-types';
import { OUTCOME_LABEL, outcomeTone } from '../logs/filters';

/** `search` with `attempt` set (or dropped when empty); other keys keep their order. */
export function withAttempt(search: string, attempt: string): string {
  const query = new URLSearchParams(search);
  if (attempt) query.set('attempt', attempt);
  else query.delete('attempt');
  const text = query.toString();
  return text ? `?${text}` : '';
}

export const attemptOf = (search: string): string => new URLSearchParams(search).get('attempt') ?? '';

/** "accepted", "rejected (factual term)", "unavailable (budget_exceeded)". */
export function verdictText(row: Pick<RewriteRow, 'verdict' | 'rule' | 'code'>): string {
  const word = OUTCOME_LABEL[row.verdict] ?? row.verdict;
  const why = row.rule ?? row.code;
  return why ? `${word} (${why})` : word;
}

export const verdictTone = (verdict: string) => outcomeTone(verdict);

/**
 * The token check that applied: "reserved 16,191 > budget 16,384" when the
 * runner refused the reservation before sending; "used 412 > reserved 287"
 * when the reply overran it (the runner refuses such a reply); "used 200 of
 * 287 reserved" otherwise; the reservation alone when nothing was reported;
 * null when nothing was sent.
 */
export function budget(row: Pick<RewriteRow, 'prompt_tokens' | 'completion_tokens' | 'reservation' | 'budget'>): string | null {
  const n = (value: number) => value.toLocaleString('en');
  if (row.reservation != null && row.budget != null) return `reserved ${n(row.reservation)} > budget ${n(row.budget)}`;
  const used = row.prompt_tokens != null && row.completion_tokens != null ? row.prompt_tokens + row.completion_tokens : null;
  if (row.reservation == null) return used == null ? null : `used ${n(used)}`;
  if (used == null) return `reserved ${n(row.reservation)}`;
  return used > row.reservation ? `used ${n(used)} > reserved ${n(row.reservation)}` : `used ${n(used)} of ${n(row.reservation)} reserved`;
}

/** Whether a token check refused the call (an overrun reply or a reservation past the budget). */
export const overran = (row: Pick<RewriteRow, 'prompt_tokens' | 'completion_tokens' | 'reservation' | 'budget'>): boolean =>
  row.budget != null ||
  (row.prompt_tokens != null && row.completion_tokens != null && row.reservation != null && row.prompt_tokens + row.completion_tokens > row.reservation);
