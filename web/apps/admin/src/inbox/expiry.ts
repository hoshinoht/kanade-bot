import type { Proposal } from '@kanade/api-types';
import { spanWords, wallMinutes, whenMinutes } from '../shared/wall';

/** The last fifth of a proposal's life takes the warning colour (and says so). */
export const EXPIRY_WARN = 0.2;

export interface Expiry {
  /** Minutes left, the bar's value (it drains toward expiry). */
  left: number;
  /** Minutes from when it was read to when it expires, the bar's max. */
  span: number;
  warn: boolean;
  text: string;
}

/**
 * How long an open item has before it expires, from the API's `read_at` and
 * `expires_at` labels against the server's clock; null without an expiry,
 * once expired, or when a label cannot be read.
 */
export function proposalExpiry(p: Pick<Proposal, 'read_at' | 'expires_at' | 'flags'>, nowIso: string | undefined, timeZone: string): Expiry | null {
  if (!p.expires_at || !nowIso || p.flags.includes('expired')) return null;
  const now = wallMinutes(nowIso, timeZone);
  if (now === null) return null;
  const end = whenMinutes(p.expires_at, now);
  const start = whenMinutes(p.read_at, now);
  if (end === null || start === null || end <= start || end <= now) return null;
  const span = end - start;
  const left = Math.min(span, end - now);
  const warn = left <= span * EXPIRY_WARN;
  return { left, span, warn, text: `${spanWords(left)} left${warn ? ', expiring soon' : ''}` };
}
