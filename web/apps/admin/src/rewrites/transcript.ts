/**
 * "Copy transcript" for one rewrite attempt, as Markdown or JSON, built only
 * from GET /api/admin/rewrites/{id}. Unknown counts and timings read
 * "unknown" (null in JSON), never 0; absent ids are left out of the Markdown.
 */
import type { Rewrite } from '@kanade/api-types';
import { KIND_LABEL, STAGE_LABEL } from '../logs/filters';
import { logTime, took } from '../logs/format';
import { count, fence, jsonDocument, type TranscriptContext } from '../logs/transcript';
import { budget, verdictText } from './format';

function header(r: Rewrite, ctx: TranscriptContext) {
  return {
    id: r.id,
    short_id: r.short_id,
    at: r.at,
    when: logTime(r.at, ctx.timeZone).title,
    kind: r.kind,
    stage: r.stage,
    context: r.context,
    verdict: r.verdict,
    rule: r.rule,
    code: r.code,
    model: r.model,
    effort: r.reasoning,
    latency_ms: r.latency_ms,
    prompt_tokens: r.prompt_tokens,
    completion_tokens: r.completion_tokens,
    reasoning_tokens: r.reasoning_tokens,
    reservation: r.reservation,
    budget: r.budget,
    max_output_tokens: r.max_output_tokens,
    prompt_estimate: r.prompt_estimate,
    // The token check in words ("used 412 > reserved 287"); null when nothing was sent.
    token_check: budget(r),
    request_id: r.request_id,
  };
}

export function rewriteMarkdown(r: Rewrite, ctx: TranscriptContext): string {
  const h = header(r, ctx);
  const lines = [
    `# Rewrite ${h.id} (#${h.short_id})`,
    '',
    `- When: ${h.when} (${h.at})`,
    `- Kind: ${KIND_LABEL[h.kind] ?? h.kind}`,
    `- Stage: ${STAGE_LABEL[h.stage] ?? h.stage}`,
    `- Context: ${h.context ?? '—'}`,
    `- Verdict: ${verdictText(r)}`,
    `- Model: ${h.model ?? 'no model call'}`,
    `- Effort: ${h.effort ?? '—'}`,
    `- Latency: ${took(h.latency_ms)}`,
    `- Tokens: prompt ${count(h.prompt_tokens)}, completion ${count(h.completion_tokens)}`,
    ...(h.reasoning_tokens !== null ? [`- Reasoning tokens: ${count(h.reasoning_tokens)}`] : []),
    ...(h.token_check ? [`- Token check: ${h.token_check}`] : []),
    `- Max tokens: ${count(h.max_output_tokens)}`,
    `- Prompt estimate: ${count(h.prompt_estimate)}`,
    ...(h.request_id ? [`- Request id: ${h.request_id}`] : []),
    '',
    '## Seed',
    '',
    fence(r.seed),
    '',
    '## Reply',
    '',
    r.reply ? fence(r.reply) : '— nothing came back —',
    '',
    '## Line used',
    '',
    r.line ? fence(r.line) : '—',
  ];
  if (r.reasoning_content) lines.push('', '## Reasoning', '', fence(r.reasoning_content));
  return `${lines.join('\n')}\n`;
}

export function rewriteJson(r: Rewrite, ctx: TranscriptContext): string {
  return jsonDocument({
    ...header(r, ctx),
    seed: r.seed,
    reply: r.reply,
    line: r.line,
    reasoning_content: r.reasoning_content,
  });
}
