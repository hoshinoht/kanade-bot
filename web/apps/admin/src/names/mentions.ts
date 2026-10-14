/**
 * Discord mention tokens in message text: `<@id>` / `<@!id>` (member),
 * `<#id>` (channel), `<@&id>` (role). Ids are Discord snowflakes; word
 * characters and `-` are accepted too so fixtures can use readable ids.
 */
export type NameKind = 'member' | 'channel' | 'role';

export type Segment = { kind: 'text'; text: string } | { kind: NameKind; id: string };

const TOKEN = /<(@!?|#|@&)([\w-]+)>/g;
const KIND: Record<string, NameKind> = { '@': 'member', '@!': 'member', '#': 'channel', '@&': 'role' };

export function parseMentions(text: string): Segment[] {
  const out: Segment[] = [];
  let last = 0;
  for (const match of text.matchAll(TOKEN)) {
    const at = match.index;
    if (at > last) out.push({ kind: 'text', text: text.slice(last, at) });
    out.push({ kind: KIND[match[1]!]!, id: match[2]! });
    last = at + match[0].length;
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) });
  return out;
}

/** The member a chatbot question opens with: Kanade itself, as mentioning it is what asks. */
export function leadingMember(text: string): string | null {
  const first = parseMentions(text.trimStart())[0];
  return first?.kind === 'member' ? first.id : null;
}
