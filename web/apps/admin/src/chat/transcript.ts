/**
 * "Copy transcript": one chat turn as Markdown or JSON for agent debugging,
 * built only from GET /api/admin/chat/{id}. People and channels are named
 * (never ids); mention tokens in the question and reply read as names.
 */
import type { ChatTurn } from '@kanade/api-types';
import { duration, logTime } from '../logs/format';
import { directory } from '../names/directory.svelte';
import { parseMentions } from '../names/mentions';

export interface TranscriptContext {
  timeZone: string;
}

type Call = ChatTurn['tools'][number];

export interface Round {
  round: number;
  /** The round's model when the API lists one per round; else null. */
  model: string | null;
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
 * Rounds with their tool calls. The API lists calls flat, in round order, and
 * each round names the tools it requested, so calls are dealt out by that
 * count; any left over belong to the last round.
 */
export function rounds(turn: ChatTurn): Round[] {
  const perRound = turn.models.length === turn.rounds.length;
  let next = 0;
  const out = turn.rounds.map((r, i) => {
    const calls = turn.tools.slice(next, next + r.requested_tools.length);
    next += calls.length;
    return { round: r.round, model: perRound ? turn.models[i]! : null, finish: r.finish, requested_tools: r.requested_tools, calls };
  });
  if (next < turn.tools.length && out.length) out[out.length - 1]!.calls.push(...turn.tools.slice(next));
  return out;
}

function header(turn: ChatTurn, ctx: TranscriptContext) {
  return {
    id: turn.id,
    at: turn.at,
    when: logTime(turn.at, ctx.timeZone).title,
    who: directory.label('member', turn.member_id || turn.member.id, turn.member.name),
    channel: turn.channel_id ? directory.label('channel', turn.channel_id, turn.channel ?? '') : null,
    outcome: turn.outcome,
    latency_ms: turn.latency_ms,
    models: [...new Set(turn.models)],
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
    lines.push('', `## Round ${r.round}${r.model ? ` — ${r.model}` : ''}`, '', `- Finish: ${r.finish || '—'}`, `- Requested tools: ${r.requested_tools.join(', ') || 'none'}`);
    for (const c of r.calls) {
      lines.push('', `### ${c.name} — ${c.outcome || '—'}, ${duration(c.took_ms)}`, '', 'Arguments:', '', fence(c.arguments, 'json'), '', 'Result:', '', fence(c.result));
    }
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
      cards: turn.cards,
      raw: turn.raw,
    },
    null,
    2,
  )}\n`;
}
