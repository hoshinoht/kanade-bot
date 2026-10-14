/**
 * Account page words and pure helpers (boards AdminAccount / AdminPhone,
 * direction B): sign-in method names, the access lines, the diagnostics text
 * and the reply-style filter. Session times and device names are shared
 * (`@kanade/ui` `account.ts`).
 */

import type { Me, Persona, ReplyStyle } from '@kanade/api-types';

export type AccountTab = 'profile' | 'sessions' | 'browser';

export const TABS: { id: AccountTab; label: string }[] = [
  { id: 'profile', label: 'Profile' },
  { id: 'sessions', label: 'Sessions' },
  { id: 'browser', label: 'This browser' },
];

export function tabOf(value: string | null | undefined): AccountTab {
  return TABS.some((t) => t.id === value) ? (value as AccountTab) : 'profile';
}

export const METHOD: Record<string, string> = { discord: 'Discord', tailscale: 'Tailscale', token: 'Token' };

/** "signed in with Discord" / "with Tailscale" / "with a token". */
export function methodLong(method: string): string {
  if (method === 'token') return 'signed in with a token';
  return `signed in with ${METHOD[method] ?? method}`;
}

export const ACCESS: Record<string, { label: string; sub: string }> = {
  staff: { label: 'Staff', sub: 'Exempt from chatbot budgets' },
  pilot: { label: 'Pilot', sub: 'Answers count against your allowance' },
  none: { label: 'None', sub: "Kanade doesn't answer you in chat" },
};

/** Guild-local "Tue 29 Sep 12:00" for the diagnostics. */
export function serverClock(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const parts = new Intl.DateTimeFormat('en-US', { timeZone, weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(date);
  const part = (type: string) => parts.find((p) => p.type === type)?.value ?? '';
  return `${part('weekday')} ${part('day')} ${part('month')} ${part('hour')}:${part('minute')}`;
}

/** What "Copy diagnostics" puts on the clipboard: ids and versions only, no secrets. */
export function diagnostics(me: Me, timeZone: string): string {
  return [
    `User id: ${me.member?.id ?? '—'}`,
    `Method: ${me.method}`,
    `Version: ${me.version}`,
    `Server: ${serverClock(me.server_time, timeZone)} (${timeZone})`,
  ].join('\n');
}

/** The default voice: what a cleared saved style means. */
export const DEFAULT_STYLE = { key: '', name: 'Default voice', voice: 'Kanade as written' } as const;

/** The picker's key for a saved style: none, or the `default` profile, is the default voice. */
export function savedKey(style: ReplyStyle): string {
  const key = style.saved?.key ?? '';
  return key === 'default' ? '' : key;
}

/** In effect: the role's or saved profile's name, else the default voice. */
export function inEffectName(style: ReplyStyle): string {
  return style.in_effect?.name ?? DEFAULT_STYLE.name;
}

/**
 * The picker's choices: the default voice (no saved style) first, then public
 * profiles A to Z, filtered by name or voice. A profile keyed `default` is
 * left out, as the Members editor does: the default voice stands for it.
 */
export function styleChoices(personas: Persona[], query: string): Persona[] {
  const named = personas.filter((p) => p.key !== 'default').sort((a, b) => a.name.localeCompare(b.name));
  const all: Persona[] = [{ ...DEFAULT_STYLE }, ...named];
  const q = query.trim().toLowerCase();
  if (!q) return all;
  return all.filter((p) => p.name.toLowerCase().includes(q) || p.voice.toLowerCase().includes(q));
}
