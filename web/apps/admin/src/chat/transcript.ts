/**
 * "Copy transcript": one chat turn as Markdown or JSON for agent debugging,
 * built only from GET /api/admin/chat/{id}. People and channels are named
 * (never ids); mention tokens in the question and reply read as names.
 */
import type { ChatTurn } from '@kanade/api-types';
import { duration, logTime } from '../logs/format';
import { profileText, routeLabel, took } from './facts';
import { directory } from '../names/directory.svelte';
import { parseMentions } from '../names/mentions';

export interface TranscriptContext {
  timeZone: string;
}

type Call = ChatTurn['tools'][number];

export interface Round {
  round: number;
  reasoning_content: string | null;
  reasoning_tokens: number | null;
  /** The alias the round's request named; null when not recorded. */
  model: string | null;
  effort: string | null;
  route: ChatTurn['route'];
  latency_ms: number | null;
  finish: string;
  requested_tools: string[];
  calls: Call[];
}

/** Message text with mention tokens as @Name / #channel / @role. */
export function mentionsText(text: string): string {
  return parseMentions(text)
    .map((s) => (s.kind === 'text' ? s.text : directory.label(s.kind, s.id, '', true)))
    .join('');
}

/**
 * Rounds with their tool calls, grouped by each call's `round`; a call naming
 * no listed round belongs to the last one. A turn with no rounds (withheld)
 * keeps its calls in [`unrounded`].
 */
export function rounds(turn: ChatTurn): Round[] {
  const perRound = turn.models.length === turn.rounds.length;
  const out: Round[] = turn.rounds.map((r, i) => ({
    round: r.round,
    reasoning_content: r.reasoning_content ?? null,
    reasoning_tokens: r.reasoning_tokens ?? null,
    model: r.model || (perRound ? turn.models[i]! : null),
    effort: r.effort ?? null,
    route: r.route ?? null,
    latency_ms: r.latency_ms ?? null,
    finish: r.finish,
    requested_tools: r.requested_tools,
    calls: [],
  }));
  for (const call of turn.tools) (out.find((r) => r.round === call.round) ?? out[out.length - 1])?.calls.push(call);
  return out;
}

/** Tool calls of a turn that has no rounds to hold them. */
export function unrounded(turn: ChatTurn): Call[] {
  return turn.rounds.length ? [] : turn.tools;
}

const callLines = (c: Call) => ['', `### ${c.name} — ${c.outcome || '—'}, ${took(c.took_ms)}`, '', 'Arguments:', '', fence(c.arguments, 'json'), '', 'Result:', '', fence(c.result)];

function header(turn: ChatTurn, ctx: TranscriptContext) {
  return {
    id: turn.id,
    at: turn.at,
    when: logTime(turn.at, ctx.timeZone).title,
    who: directory.label('member', turn.member_id || turn.member.id, turn.member.name),
    channel: turn.channel_id ? directory.label('channel', turn.channel_id, turn.channel ?? '') : null,
    outcome: turn.outcome,
    error: turn.error ?? null,
    error_code: turn.error_code ?? null,
    latency_ms: turn.latency_ms,
    models: [...new Set(turn.models)],
    persona: turn.persona ?? null,
    profile: turn.profile ?? null,
    profile_source: turn.profile_source ?? null,
    route: turn.route ?? null,
  };
}

const fence = (text: string, lang = '') => {
  // A fence longer than any run of backticks inside, so the text cannot close it.
  const ticks = '`'.repeat(Math.max(3, ...[...text.matchAll(/`+/g)].map((m) => m[0].length + 1)));
  return `${ticks}${lang}\n${text}\n${ticks}`;
};

export function transcriptMarkdown(turn: ChatTurn, ctx: TranscriptContext): string {
  const h = header(turn, ctx);
  const lines = [
    `# Chat turn ${h.id}`,
    '',
    `- When: ${h.when} (${h.at})`,
    `- Who: ${h.who}`,
    `- Channel: ${h.channel ?? '—'}`,
    `- Outcome: ${h.outcome}`,
    ...(h.error || h.error_code ? [`- Error: ${h.error ?? '—'}${h.error_code ? ` (${h.error_code})` : ''}`] : []),
    `- Persona: ${h.persona ?? '—'}`,
    `- Reply profile: ${profileText(turn)}`,
    `- Route: ${routeLabel(h.route)}`,
    `- Models: ${h.models.join(', ') || '—'}`,
    `- Took: ${duration(turn.latency_ms)}`,
    '',
    '## Question',
    '',
    fence(mentionsText(turn.asked)),
    '',
    '## Reply',
    '',
    turn.said ? fence(mentionsText(turn.said)) : '— nothing was sent —',
  ];
  for (const r of rounds(turn)) {
    lines.push(
      '',
      `## Round ${r.round}${r.model ? ` — ${r.model}` : ''}`,
      '',
      `- Effort: ${r.effort ?? '—'}`,
      `- Route: ${routeLabel(r.route)}`,
      `- Latency: ${took(r.latency_ms)}`,
      `- Finish: ${r.finish || '—'}`,
      `- Requested tools: ${r.requested_tools.join(', ') || 'none'}`,
    );
    if (r.reasoning_tokens !== null) lines.push(`- Reasoning tokens: ${r.reasoning_tokens}`);
    if (r.reasoning_content) lines.push('', '### Reasoning', '', fence(r.reasoning_content));
    for (const c of r.calls) lines.push(...callLines(c));
  }
  const loose = unrounded(turn);
  if (loose.length) {
    lines.push('', '## Tool calls');
    for (const c of loose) lines.push(...callLines(c));
  }
  if (turn.cards.length) {
    lines.push('', '## Cards', '', ...turn.cards.map((c) => `- ${c.kind}: ${c.url}`));
  }
  if (turn.raw) lines.push('', '## Raw model output', '', fence(turn.raw));
  return `${lines.join('\n')}\n`;
}

export function transcriptJson(turn: ChatTurn, ctx: TranscriptContext): string {
  return `${JSON.stringify(
    {
      ...header(turn, ctx),
      question: mentionsText(turn.asked),
      reply: turn.said ? mentionsText(turn.said) : '',
      rounds: rounds(turn),
      ...(unrounded(turn).length ? { tool_calls: unrounded(turn) } : {}),
      cards: turn.cards,
      raw: turn.raw,
    },
    null,
    2,
  )}\n`;
}
