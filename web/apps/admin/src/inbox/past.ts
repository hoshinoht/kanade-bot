import type { PastItem, PastOutcome } from '@kanade/api-types';
import type { IconName } from '@kanade/ui';
import { localAt } from '../history/describe';

/** Each outcome as its word, chip tone and mark: a shape and words, never colour alone. */
export const OUTCOME: Record<PastOutcome, { label: string; tone: 'ok' | 'risk' | 'warn' | 'neutral'; icon: IconName }> = {
  approved: { label: 'Approved', tone: 'ok', icon: 'check' },
  rejected: { label: 'Rejected', tone: 'risk', icon: 'x' },
  expired: { label: 'Expired', tone: 'warn', icon: 'clock' },
  superseded: { label: 'Superseded', tone: 'neutral', icon: 'refresh-cw' },
  discarded: { label: 'Discarded', tone: 'neutral', icon: 'alert-circle' },
  withdrawn: { label: 'Withdrawn', tone: 'neutral', icon: 'rotate-ccw' },
};

/** Who closed it, in words; Kanade's own closes (expiry, superseding) need no name. */
export function decider(item: PastItem): string {
  if (!item.decided_by) return '';
  if (item.decided_by.kind === 'system' && (item.outcome === 'expired' || item.outcome === 'superseded')) return '';
  return item.decided_by.name;
}

/** "Approved by Asahi · Thu 24 Sep 14:00"; a superseded item names what replaced it. */
export function outcomeSentence(item: PastItem, timeZone: string): string {
  const who = decider(item);
  const what = item.outcome === 'superseded' ? 'Superseded by a newer proposal' : `${OUTCOME[item.outcome].label}${who ? ` by ${who}` : ''}`;
  return `${what} · ${localAt(item.decided_at, timeZone)}`;
}

/** The extraction or chat log entry that staged a proposal, when the app can open it. */
export function sourceLink(item: PastItem): { href: string; label: string } | null {
  if (!item.source_id) return null;
  const id = encodeURIComponent(item.source_id);
  if (item.source === 'extraction') return { href: `/extractions/${id}`, label: 'Extraction log entry' };
  if (item.source === 'chat') return { href: `/chat/${id}`, label: 'Chat interaction' };
  return null;
}
