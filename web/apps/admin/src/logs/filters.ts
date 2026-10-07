import type { Tone } from '@kanade/ui';
/**
 * Chat, Extractions and Rewrites log filters (user request 2026-09-25):
 * server-side, deep-linked through the page's query string, combinable.
 * Pure, so the parse/serialise round trip is unit-tested. Dates are
 * guild-local YYYY-MM-DD.
 */

export interface LogFilter {
  model: string;
  from: string;
  to: string;
  /** Outcomes (Rewrites: verdicts), any of. */
  outcome: string[];
  channel: string;
  member: string;
  q: string;
  tool: string;
  min_ms: string;
  /** Rewrites only. */
  kind: string;
  stage: string;
}

export const NO_LOG_FILTER: LogFilter = {
  model: '',
  from: '',
  to: '',
  outcome: [],
  channel: '',
  member: '',
  q: '',
  tool: '',
  min_ms: '',
  kind: '',
  stage: '',
};

const KEYS = ['model', 'from', 'to', 'channel', 'member', 'q', 'tool', 'min_ms', 'kind', 'stage'] as const;

const CHAT_ONLY: readonly string[] = ['tool', 'min_ms'];
const REWRITE_ONLY: readonly string[] = ['kind', 'stage'];
/** Keys the Rewrites log has no use for. */
const NOT_REWRITE: readonly string[] = ['channel', 'member', ...CHAT_ONLY];

export interface LogScope {
  /** Chat adds tool used and minimum latency (default). */
  chat?: boolean;
  /** The Rewrites log: kind and stage, no channel or member; outcomes travel as `verdict`. */
  rewrites?: boolean;
}

function keeps(key: string, { chat = true, rewrites = false }: LogScope): boolean {
  if (rewrites) return !NOT_REWRITE.includes(key);
  if (REWRITE_ONLY.includes(key)) return false;
  return chat || !CHAT_ONLY.includes(key);
}

/** Keys outside the scope are dropped, so they never show as filters that do nothing. */
export function parseFilter(search: string, scope: LogScope = {}): LogFilter {
  const params = new URLSearchParams(search);
  const out: LogFilter = { ...NO_LOG_FILTER, outcome: [] };
  for (const key of KEYS) out[key] = keeps(key, scope) ? (params.get(key) ?? '') : '';
  out.outcome = (params.get(scope.rewrites ? 'verdict' : 'outcome') ?? '').split(',').filter(Boolean);
  return out;
}

/** `?model=…&outcome=a,b` (Rewrites: `verdict=a,b`): only what is set, in a stable order. */
export function toSearch(filter: LogFilter, { rewrites = false }: { rewrites?: boolean } = {}): string {
  const params = new URLSearchParams();
  for (const key of KEYS) {
    const value = filter[key].trim();
    if (value) params.set(key, value);
  }
  if (filter.outcome.length) params.set(rewrites ? 'verdict' : 'outcome', filter.outcome.join(','));
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
  accepted: 'accepted',
  rejected: 'rejected',
  unavailable: 'unavailable',
  misconfigured: 'misconfigured',
  no_rewriter: 'no rewriter',
  no_persona: 'no persona',
};

/** Rewrites: which line, and where it ran. */
export const KIND_LABEL: Record<string, string> = {
  day_of: 'day-of',
  countdown: 'countdown',
  digest: 'digest',
  nudge: 'nudge',
};

export const STAGE_LABEL: Record<string, string> = {
  batch: 'daily batch',
  catchup: 'catch-up',
  debug: '/debug',
  nudge: 'self-service nudge',
  manual: 'manual',
};

/** The pill profile for a log outcome (the word is always shown too). */
export function outcomeTone(outcome: string): Tone {
  if (['answered', 'proposed', 'accepted'].includes(outcome)) return 'success';
  if (['error', 'timeout', 'failed', 'content_blocked', 'unavailable', 'misconfigured'].includes(outcome)) return 'danger';
  if (['clarified', 'clean_retry', 'self_service_link'].includes(outcome)) return 'info';
  if (['no_change', 'withheld', 'no_rewriter', 'no_persona'].includes(outcome)) return 'neutral';
  return 'warning';
}
