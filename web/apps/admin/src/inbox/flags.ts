import type { Proposal, ProposalFlag } from '@kanade/api-types';

/** Badge words; each badge is text, never colour alone. */
export const FLAG_LABEL: Record<ProposalFlag, string> = {
  conflict: 'conflict',
  expired: 'expired',
  requester_frozen: 'frozen requester',
  no_effect: 'already in effect',
};

export const FLAG_TONE: Record<ProposalFlag, string> = {
  conflict: 'chip--no',
  expired: 'chip--waiting',
  requester_frozen: 'chip--maybe',
  no_effect: 'chip--waiting',
};

export const title = (p: Proposal) => `${p.kind_label} — ${p.bosses.map((b) => b.token).join(' + ')}`;

/** Who it came from: the member for requests, the first person quoted for the extractor. */
export function who(p: Proposal): string {
  if (p.self_service) return p.self_service.member.name;
  return p.evidence.find((e) => !e.missing)?.author ?? p.channel ?? 'the extractor';
}

/** Why approve is unavailable, in words; empty when it is allowed. */
export function blocked(p: Proposal): string {
  if (p.flags.includes('expired')) return 'It expired; it can only be rejected.';
  if (p.flags.includes('no_effect')) return 'It is already like that, so approving would change nothing.';
  return '';
}

export const REASON_MAX = 500;

/** A reject reason: required (1–500 characters) for member requests. */
export function reasonProblem(p: Proposal, reason: string): string {
  const text = reason.trim();
  if (p.tab === 'self_service' && !text) return 'Say why, in a sentence: the member is told.';
  if ([...text].length > REASON_MAX) return `A reason is at most ${REASON_MAX} characters.`;
  return '';
}
