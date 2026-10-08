/**
 * "Copy transcript" for one extraction call, as Markdown or JSON, built only
 * from GET /api/admin/extractions/{id}. Authors and the channel are named
 * (never ids) and mention tokens in the messages read as names; unknown
 * counts and timings read "unknown" (null in JSON), never 0.
 */
import type { Extraction } from '@kanade/api-types';
import { OUTCOME_LABEL } from '../logs/filters';
import { logTime, took } from '../logs/format';
import { count, fence, jsonDocument, mentionsText, type TranscriptContext } from '../logs/transcript';
import { directory } from '../names/directory.svelte';
import { pretty } from './code';

/** The server's stored response when the call failed before one came back. */
const NO_RESPONSE = 'null';

function header(call: Extraction, ctx: TranscriptContext) {
  return {
    id: call.id,
    short_id: call.short_id,
    at: call.at,
    when: logTime(call.at, ctx.timeZone).title,
    channel: call.channel_id ? directory.label('channel', call.channel_id, call.channel ?? '') : null,
    model: call.model,
    outcome: call.outcome,
    error: call.error,
    latency_ms: call.latency_ms,
    prompt_tokens: call.prompt_tokens ?? null,
    completion_tokens: call.completion_tokens ?? null,
    reasoning_tokens: call.reasoning_tokens ?? null,
    prompt_estimate: call.prompt_estimate ?? null,
    context: call.context ?? null,
    session_id: call.session_id ?? null,
    request_ids: call.request_ids ?? [],
  };
}

function messages(call: Extraction, ctx: TranscriptContext) {
  return call.messages.map((m) => ({
    who: directory.label('member', m.author_id ?? '', m.author),
    at: m.at,
    when: logTime(m.at, ctx.timeZone).title,
    content: mentionsText(m.content),
  }));
}

export function extractionMarkdown(call: Extraction, ctx: TranscriptContext): string {
  const h = header(call, ctx);
  const read = messages(call, ctx);
  const refusals = call.refusals ?? [];
  const lines = [
    `# Extraction ${h.id} (#${h.short_id})`,
    '',
    `- When: ${h.when} (${h.at})`,
    `- Channel: ${h.channel ?? '—'}`,
    `- Model: ${h.model}`,
    `- Outcome: ${OUTCOME_LABEL[h.outcome] ?? h.outcome}`,
    ...(h.error ? [`- Error: ${h.error}`] : []),
    `- Latency: ${took(h.latency_ms)}`,
    `- Tokens: prompt ${count(h.prompt_tokens)}, completion ${count(h.completion_tokens)}`,
    ...(h.reasoning_tokens !== null ? [`- Reasoning tokens: ${count(h.reasoning_tokens)}`] : []),
    `- Prompt estimate: ${count(h.prompt_estimate)}`,
    `- Context: ${h.context ? `window ${count(h.context.window)}, reserve ${count(h.context.reserve)} (${h.context.source})${typeof h.context.sent_max_tokens === 'number' ? `, max tokens sent ${count(h.context.sent_max_tokens)}` : ''}` : '—'}`,
    ...(h.session_id ? [`- Gateway session: ${h.session_id}`] : []),
    ...(h.request_ids.length ? [`- Request ids: ${h.request_ids.join(', ')}`] : []),
    '',
    `## Messages read (${read.length})`,
    '',
    read.length ? fence(read.map((m) => `[${m.when}] ${m.who}: ${m.content}`).join('\n')) : '— no messages were stored —',
    '',
    `## Changes proposed (${call.amendments.length})`,
    '',
    ...(call.amendments.length ? call.amendments.map((a) => `- ${a.kind} · ${a.bosses} · ${a.when} · confidence ${a.confidence.toFixed(2)} · ${a.status}`) : ['none']),
  ];
  if (refusals.length) lines.push('', `## Refused changes (${refusals.length})`, '', ...refusals.map((r) => `- ${r.change} — ${r.code}: ${r.message}`));
  if (call.reasoning_content) lines.push('', '## Reasoning', '', fence(call.reasoning_content));
  lines.push('', '## Prompt as sent', '', fence(call.prompt));
  lines.push('', '## Raw response', '', call.raw_response === NO_RESPONSE ? '— no response (the call failed) —' : fence(pretty(call.raw_response)));
  return `${lines.join('\n')}\n`;
}

export function extractionJson(call: Extraction, ctx: TranscriptContext): string {
  return jsonDocument({
    ...header(call, ctx),
    messages: messages(call, ctx),
    amendments: call.amendments,
    refusals: call.refusals ?? [],
    reasoning_content: call.reasoning_content ?? null,
    prompt: call.prompt,
    raw_response: call.raw_response === NO_RESPONSE ? null : call.raw_response,
  });
}
