import type { OwnershipRequest } from '@kanade/api-types';
import { spanWords } from '@kanade/ui';
import { directory } from '../names/directory.svelte';

/** The timing a request is about: its bosses, as the other inbox rows lead. */
export const ownershipTitle = (r: OwnershipRequest) => `Ownership — ${r.bosses.map((b) => b.token).join(' + ') || `#${r.fixed_short_id}`}`;

/** "Ren → Asahi": who asks, then who owns it now. */
export const handover = (r: OwnershipRequest) =>
  `${directory.label('member', r.requester.id, r.requester.name)} → ${directory.label('member', r.owner.id, r.owner.name)}`;

/** Minutes until it expires on the server's clock; null when unreadable. */
export function minutesLeft(r: Pick<OwnershipRequest, 'expires_at'>, nowIso: string): number | null {
  const now = Date.parse(nowIso);
  const end = Date.parse(r.expires_at);
  if (Number.isNaN(now) || Number.isNaN(end)) return null;
  return Math.max(0, (end - now) / 60_000);
}

/** "expires in 4 h", or empty without the server's clock. */
export function expiresIn(r: Pick<OwnershipRequest, 'expires_at'>, nowIso: string): string {
  const left = minutesLeft(r, nowIso);
  if (left === null) return '';
  return left > 0 ? `expires in ${spanWords(left)}` : 'expired';
}
