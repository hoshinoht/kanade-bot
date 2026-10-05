import type { Tone } from '@kanade/ui';
/**
 * Chat and Extractions log filters (user request 2026-09-25): server-side,
 * deep-linked through the page's query string, combinable. Pure, so the
 * parse/serialise round trip is unit-tested. Dates are guild-local YYYY-MM-DD.
 */

export interface LogFilter {
  model: string;
  from: string;
  to: string;
  outcome: string[];
  channel: string;
  member: string;
  q: string;
  tool: string;
  min_ms: string;
}

export const NO_LOG_FILTER: LogFilter = { model: '', from: '', to: '', outcome: [], channel: '', member: '', q: '', tool: '', min_ms: '' };

const KEYS = ['model', 'from', 'to', 'channel', 'member', 'q', 'tool', 'min_ms'] as const;

const CHAT_ONLY: readonly string[] = ['tool', 'min_ms'];

/** `chat: false` (Extractions) drops Chat-only keys, so they never show as filters that do nothing. */
export function parseFilter(search: string, { chat = true }: { chat?: boolean } = {}): LogFilter {
  const params = new URLSearchParams(search);
  const out: LogFilter = { ...NO_LOG_FILTER, outcome: [] };
  for (const key of KEYS) out[key] = chat || !CHAT_ONLY.includes(key) ? (params.get(key) ?? '') : '';
  out.outcome = (params.get('outcome') ?? '').split(',').filter(Boolean);
  return out;
}

/** `?model=…&outcome=a,b`: only what is set, in a stable order. */
export function toSearch(filter: LogFilter): string {
  const params = new URLSearchParams();
  for (const key of KEYS) {
    const value = filter[key].trim();
    if (value) params.set(key, value);
  }
  if (filter.outcome.length) params.set('outcome', filter.outcome.join(','));
  const text = params.toString();
  return text ? `?${text}` : '';
}

/** How many filters are on (text search included). */
export function activeCount(filter: LogFilter): number {
  return KEYS.filter((k) => filter[k].trim()).length + (filter.outcome.length ? 1 : 0);
}

export const OUTCOME_LABEL: Record<string, string> = {
  answered: 'answered',
  refused: 'refused',
  clarified: 'clarified',
  error: 'error',
  timeout: 'timeout',
  rate_limited: 'rate-limited',
  turned_away: 'turned away',
  content_blocked: 'content-blocked',
  withheld: 'withheld',
  clean_retry: 'clean retry',
  proposed: 'proposed',
  no_change: 'no change',
  failed: 'failed',
  self_service_link: 'self-service link sent',
  identity_leak: 'identity leak blocked',
  profanity: 'profanity',
};

/** The pill profile for a log outcome (the word is always shown too). */
export function outcomeTone(outcome: string): Tone {
  if (['answered', 'proposed'].includes(outcome)) return 'success';
  if (['error', 'timeout', 'failed', 'content_blocked'].includes(outcome)) return 'danger';
  if (['clarified', 'clean_retry', 'self_service_link'].includes(outcome)) return 'info';
  if (['no_change', 'withheld'].includes(outcome)) return 'neutral';
  return 'warning';
}
