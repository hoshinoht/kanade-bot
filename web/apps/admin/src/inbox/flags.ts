import type { Proposal, ProposalFlag } from '@kanade/api-types';

/** Badge words; each badge is text, never colour alone. */
export const FLAG_LABEL: Record<ProposalFlag, string> = {
  conflict: 'conflict',
  expired: 'expired',
  requester_frozen: 'frozen requester',
  requester_unauthorised: 'requester not allowed',
  no_effect: 'already in effect',
};

export const FLAG_TONE: Record<ProposalFlag, string> = {
  conflict: 'chip--no',
  expired: 'chip--waiting',
  requester_frozen: 'chip--maybe',
  requester_unauthorised: 'chip--no',
  no_effect: 'chip--waiting',
};

/** Where an item came from, in words. */
export const SOURCE_LABEL: Record<Proposal['source'], string> = {
  extraction: 'Read from chat',
  chat: 'Asked of Kanade',
  self_service: 'Member request',
};

/** A proposal is Kanade's (decided like a ✅ on its card); a request is a member's. */
export const isProposal = (p: Proposal) => p.tab === 'extractor';

export const DISCORD_ONLY = "Sign in with Discord to approve or reject Kanade's proposals.";

export const title = (p: Proposal) => `${p.kind_label} — ${p.bosses.map((b) => b.token).join(' + ')}`;

/** Who it came from: the member for requests, the first person quoted for the extractor. */
export function who(p: Proposal): string {
  if (p.self_service) return p.self_service.member.name;
  return p.evidence.find((e) => !e.missing)?.author ?? p.channel ?? 'the extractor';
}

/** Why approve is unavailable, in words; empty when it is allowed. Conflicts always block. */
export function blocked(p: Proposal): string {
  if (p.flags.includes('expired')) return 'It expired; it can only be rejected.';
  if (p.flags.includes('requester_unauthorised')) return 'The member may no longer have this approved; reject it.';
  if (p.preview.conflicts.length) return 'It changed since it was asked, so it cannot be approved; reject it.';
  if (p.flags.includes('no_effect')) return 'It is already like that, so approving would change nothing.';
  return '';
}

/** Server refusals where the app knows better words or a next step; others keep the server's message. */
const REFUSAL: Record<string, string> = {
  discord_session_required: DISCORD_ONLY,
  edit_not_applicable: 'Only a move or a new run can have its time edited; approve it as it is.',
  reason_not_applicable: "Kanade's proposals are rejected without a reason.",
  requester_unauthorised: 'The member may no longer have this approved; reject it.',
  version_required: 'This needs the version you reviewed; reload the inbox and try again.',
  force_unsupported: 'Conflicts cannot be approved over; reject it instead.',
  stale: 'It changed or was decided since you opened it; the inbox has been reloaded.',
};

export function refusalText(code: string | null | undefined, message: string): string {
  return (code && REFUSAL[code]) || message;
}

export const REASON_MAX = 500;

/** A reject reason: required (1–500 characters) for member requests; proposals take none. */
export function reasonProblem(p: Proposal, reason: string): string {
  if (isProposal(p)) return '';
  const text = reason.trim();
  if (p.tab === 'self_service' && !text) return 'Say why, in a sentence: the member is told.';
  if ([...text].length > REASON_MAX) return `A reason is at most ${REASON_MAX} characters.`;
  return '';
}
