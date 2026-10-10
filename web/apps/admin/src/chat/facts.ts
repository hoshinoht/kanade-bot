/**
 * Chat turn facts in words: routes, reply profiles and the Model view's
 * request messages. Pure, so unit-tested.
 */
import type { ChatRoute, ChatTurn, ModelView } from '@kanade/api-types';
import { outcomeTone } from '../logs/filters';

export const ROUTE_LABEL: Record<ChatRoute, string> = {
  homelab: 'Homelab',
  external_masked: 'External (masked)',
  external_unmasked: 'External (unmasked)',
};

export const routeLabel = (route: ChatRoute | null): string => (route ? (ROUTE_LABEL[route] ?? route) : '—');

export type ChipTone = 'neutral' | 'ok' | 'risk' | 'warn';

/** The M3E chip for an outcome (`.ok2` / `.err2`); the legacy pill tone rides along. */
export function chipTone(outcome: string): ChipTone {
  const tone = outcomeTone(outcome);
  return tone === 'success' ? 'ok' : tone === 'danger' ? 'risk' : tone === 'warning' ? 'warn' : 'neutral';
}

/** A tool call's outcome chip: "ok" or what stopped it. */
export const callTone = (outcome: string): ChipTone => (outcome === 'ok' ? 'ok' : 'risk');

/** Route chips: leaving the homelab is a warning tint, the homelab an ok tint (B_ChatTrace). */
export const routeTone = (route: ChatRoute | null): ChipTone => (route === 'homelab' ? 'ok' : route ? 'warn' : 'neutral');

const SOURCE: Record<NonNullable<ChatTurn['profile_source']>, string> = { saved: 'saved', role: 'from role', default: 'default' };

/** "gentle (saved)", "default voice (default)"; "—" when no persona answered. */
export function profileText(turn: Pick<ChatTurn, 'profile' | 'profile_source'>): string {
  if (!turn.profile && !turn.profile_source) return '—';
  const name = turn.profile ?? 'default voice';
  return turn.profile_source ? `${name} (${SOURCE[turn.profile_source] ?? turn.profile_source})` : name;
}

/** The guardrail flags that fired, e.g. ["pseudonymized", "content filter"]. */
export function guardrailFlags(guardrail: Record<string, unknown> | null | undefined): string[] {
  return Object.entries(guardrail ?? {})
    .filter(([, v]) => v !== false && v !== null && v !== undefined)
    .map(([k]) => k.replace(/_/g, ' '));
}

export type ModelViewState = 'none' | 'withheld' | 'shown';

/** Passthrough turns show nothing; masked-but-withheld turns say why there is nothing. */
export function modelViewState(turn: Pick<ChatTurn, 'masked' | 'model_view'>): ModelViewState {
  if (!turn.masked) return 'none';
  return turn.model_view ? 'shown' : 'withheld';
}

type Message = ModelView['rounds'][number]['request'][number];

export interface MessageParts {
  role: string;
  /** The message text as sent; null when it had none (an assistant tool call). */
  content: string | null;
  /** Everything else on the message (tool calls, tool_call_id) as JSON; null when nothing. */
  extra: string | null;
}

export function messageParts(message: Message): MessageParts {
  const { role, content, ...rest } = message;
  const text = content === null || content === undefined ? null : typeof content === 'string' ? content : JSON.stringify(content, null, 2);
  return { role: String(role), content: text, extra: Object.keys(rest).length ? JSON.stringify(rest, null, 2) : null };
}
