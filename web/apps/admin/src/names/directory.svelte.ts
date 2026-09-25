/**
 * Display names for Discord members, channels and roles (user decision
 * 2026-09-26: ids are never shown as raw numbers). Names come from the
 * member list, the channel list, role profiles and the bot identity; a hint
 * the server sent is used when it is a real name, and a neutral fallback
 * otherwise — never the id.
 */
import type { Channel, Identity, Member, RoleProfile } from '@kanade/api-types';
import { SvelteMap } from 'svelte/reactivity';
import type { NameKind } from './mentions';

export const UNKNOWN: Record<NameKind, string> = {
  member: 'Unknown member',
  channel: '#unknown-channel',
  role: '@unknown-role',
};

/** Placeholders the server sends when it has no name (`user 11494860`, the id itself). */
function placeholder(kind: NameKind, id: string, hint: string): boolean {
  const bare = hint.replace(/^[#@]/, '').trim();
  // A bare number is the id echoed back (Discord ids are snowflakes).
  if (!bare || /^\d+$/.test(bare) || bare === 'unknown' || bare === '?') return true;
  return kind === 'member' && /^user [0-9a-f]+$/i.test(bare);
}

export interface Known {
  members: Map<string, string>;
  channels: Map<string, string>;
  roles: Map<string, string>;
  bot: { id: string | null; name: string };
}

/** The name to show; `mention` adds the `@`/`#` a Discord mention reads with. */
export function resolveName(known: Known, kind: NameKind, id: string, hint = '', mention = false): string {
  let name: string | undefined;
  if (kind === 'member' && id && id === known.bot.id) name = known.bot.name;
  name ??= (kind === 'member' ? known.members : kind === 'channel' ? known.channels : known.roles).get(id);
  if (!name && hint && !placeholder(kind, id, hint)) name = hint;
  if (!name) return UNKNOWN[kind];
  const bare = name.replace(/^[#@]/, '');
  if (kind === 'channel') return `#${bare}`;
  if (kind === 'role') return `@${bare}`;
  return mention ? `@${bare}` : name;
}

class Directory implements Known {
  members = new SvelteMap<string, string>();
  channels = new SvelteMap<string, string>();
  roles = new SvelteMap<string, string>();
  bot = $state<{ id: string | null; name: string }>({ id: null, name: 'Kanade' });

  setMembers(list: Member[]) {
    for (const m of list) if (!placeholder('member', m.id, m.name)) this.members.set(m.id, m.name);
  }

  setChannels(list: Channel[]) {
    for (const c of list) if (!placeholder('channel', c.id, c.name)) this.channels.set(c.id, c.name);
  }

  setRoles(list: RoleProfile[]) {
    for (const r of list) if (r.role_id && !placeholder('role', r.role_id, r.role_name)) this.roles.set(r.role_id, r.role_name);
  }

  setIdentity(identity: Identity | null) {
    if (identity) this.bot.name = identity.name;
  }

  /**
   * The API does not send the bot's user id yet; a chatbot question opens by
   * mentioning Kanade, so an unknown member leading one is taken to be it.
   */
  noteBot(id: string) {
    if (!this.bot.id && !this.members.has(id)) this.bot.id = id;
  }

  label(kind: NameKind, id: string, hint = '', mention = false): string {
    return resolveName(this, kind, id, hint, mention);
  }
}

export const directory = new Directory();

/**
 * A member's name for lists where two people can share one: a twin gets its
 * place among the twins ("Ren (2)"), never its id.
 */
export function memberLabel(members: Member[], id: string): string {
  const name = directory.label('member', id, members.find((m) => m.id === id)?.name ?? '');
  const twins = members.filter((m) => directory.label('member', m.id, m.name) === name);
  if (twins.length < 2) return name;
  return `${name} (${twins.findIndex((m) => m.id === id) + 1})`;
}
