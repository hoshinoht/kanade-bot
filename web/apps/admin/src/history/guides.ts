/**
 * Guide changes on the History timeline (section `guides`, approved Guides
 * default 10, board G_History): view-only rows beside the Config saves.
 * Keys: `guide.<id>` (created, renamed, channel, deleted), `publish.<run>`
 * (the admin's request, then `system:guides`' outcome) and
 * `slot.<guide>.<position>` (Mark as not posted). Rows name the guide by
 * title: from the guide list, else the guide's own create/rename rows.
 */
import type { SettingsChangeRow } from '@kanade/api-types';
import type { SettingField } from './settings';

export const GUIDES_SECTION = 'guides';
export const isGuideChange = (change: SettingsChangeRow) => change.section === GUIDES_SECTION;

/** What the page knows of guides now: titles by id and the guide using each channel. */
export interface GuideLookup {
  titles: Record<string, string>;
  byChannel: Record<string, string>;
  channelName: (id: string) => string;
}

export interface GuideEvent {
  /** The guide's id, when known (the pane links to it). */
  guide: string | null;
  title: string;
  /** "Guides · Limbo party roles — publish requested: add 7". */
  summary: string;
  /** The row's count chip: "publish", "outcome", "guide", "1 message". */
  count: string;
  /** The pane's heading: "Limbo party roles: publish requested". */
  head: string;
  /** The pane's meta after "by …": "version 1 · add 7". */
  meta: string;
  fields: SettingField[];
}

const parse = (text: string): Record<string, unknown> => {
  try {
    const value: unknown = JSON.parse(text);
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
};
const num = (value: unknown) => (typeof value === 'number' ? value : 0);
const str = (value: unknown) => (typeof value === 'string' ? value : null);

const STOPPED: Record<string, string> = {
  plan_changed: 'the plan changed',
  unknown_slot: 'a message unconfirmed',
  edit_unconfirmed: 'an edit unconfirmed',
  delete_unconfirmed: 'a delete unconfirmed',
  rejected: 'Discord refused',
  discord_unavailable: 'Discord unavailable',
  interrupted: 'Kanade restarted',
};

/** "add 7, edit 2" from a request's planned counts. */
function planned(ops: Record<string, unknown>): string {
  const parts = [
    ['add', num(ops.create)],
    ['edit', num(ops.edit)],
    ['delete', num(ops.delete)],
  ]
    .filter(([, n]) => n)
    .map(([word, n]) => `${word} ${n}`);
  return parts.join(', ') || 'nothing to change';
}

/** "7 added, 2 edited": a done run's planned counts, in the past. */
function done(ops: Record<string, unknown>): string {
  const parts = [
    [num(ops.create), 'added'],
    [num(ops.edit), 'edited'],
    [num(ops.delete), 'deleted'],
  ]
    .filter(([n]) => n)
    .map(([n, word]) => `${n} ${word}`);
  return parts.join(', ') || 'nothing changed';
}

/** A guide change in History's words; `null` for another section. */
export function guideEvent(change: SettingsChangeRow, all: readonly SettingsChangeRow[], lookup: GuideLookup): GuideEvent | null {
  if (!isGuideChange(change)) return null;
  const row = change.values[0];
  if (!row) return null;
  const [kind, ...rest] = row.key.split('.');
  const from = parse(row.from);
  const to = parse(row.to);
  const titleOf = (guide: string | null) => {
    if (!guide) return 'a guide';
    const named = all.find((c) => c.values[0]?.key === `guide.${guide}` && (str(parse(c.values[0].to).title) ?? str(parse(c.values[0].from).title)));
    const saved = named ? (str(parse(named.values[0]!.to).title) ?? str(parse(named.values[0]!.from).title)) : null;
    return lookup.titles[guide] ?? saved ?? 'a guide';
  };
  const event = (guide: string | null, what: string, count: string, meta: string, fields: SettingField[]): GuideEvent => {
    const title = titleOf(guide);
    return { guide, title, summary: `Guides · ${title} — ${what}`, count, head: `${title}: ${what.split(':')[0]}`, meta, fields };
  };
  const channel = (id: unknown) => (typeof id === 'string' && id ? lookup.channelName(id) : '—');

  if (kind === 'guide') {
    const guide = rest.join('.');
    const fields = [
      { name: 'title', was: str(from.title) ?? '—', now: str(to.title) ?? '—' },
      { name: 'channel', was: channel(from.channel), now: channel(to.channel) },
    ].filter((f) => f.was !== f.now);
    if (!row.from) return event(guide, 'created', 'guide', 'view only', fields);
    if (!row.to) return event(guide, 'deleted', 'guide', 'view only', fields);
    if (from.channel !== to.channel) return event(guide, `channel: ${channel(from.channel)} → ${channel(to.channel)}`, 'guide', 'view only', fields);
    return event(guide, `renamed: ${str(from.title)} → ${str(to.title)}`, 'guide', 'view only', fields);
  }

  if (kind === 'slot') {
    const position = Number(rest[rest.length - 1]);
    const guide = rest.slice(0, -1).join('.');
    return event(guide, `message ${position + 1} marked not posted`, '1 message', 'view only', [
      { name: 'message', was: `${position + 1} (${str(from.state) ?? 'unknown'})`, now: 'not posted' },
      { name: 'channel', was: channel(from.channel), now: channel(from.channel) },
    ]);
  }

  // `publish.<run>`: the request carries the plan; the outcome, the state.
  const request = to.kind !== undefined ? to : parse(all.find((c) => c.values[0]?.key === row.key && parse(c.values[0].to).kind !== undefined)?.values[0]?.to ?? '');
  const runChannel = str(request.channel);
  const guide = runChannel ? (lookup.byChannel[runChannel] ?? null) : null;
  const ops = (request.ops ?? {}) as Record<string, unknown>;
  const runKind = str(request.kind) ?? 'publish';
  if (to.kind !== undefined) {
    const confirmed = (to.confirmed ?? {}) as Record<string, unknown>;
    const what = runKind === 'unpublish' ? `unpublish requested: ${planned(ops)}` : runKind === 'search' ? 'search requested' : `publish requested: ${planned(ops)}`;
    const ticks = [confirmed.repost_from !== null && confirmed.repost_from !== undefined && 'repost', confirmed.interleaved && 'below others', confirmed.channel_move && 'move'].filter(Boolean).join(', ');
    return event(guide, what, runKind === 'search' ? 'search' : runKind, `version ${num(to.version)} · ${planned(ops)} · view only`, [
      { name: 'channel', was: '—', now: channel(to.channel) },
      { name: 'messages', was: String(num(from.slots)), now: String(num(ops.keep) + num(ops.edit) + num(ops.create)) },
      { name: 'add', was: '0', now: String(num(ops.create)) },
      { name: 'edit · delete', was: '0 · 0', now: `${num(ops.edit)} · ${num(ops.delete)}` },
      { name: 'confirmed', was: '—', now: ticks || 'nothing needed' },
    ]);
  }
  const state = str(to.state) ?? '';
  const verb = runKind === 'unpublish' ? 'unpublished' : runKind === 'search' ? 'searched' : 'published';
  const what =
    state === 'done'
      ? runKind === 'search'
        ? 'search finished'
        : `${verb}: ${done(ops)}`
      : state === 'cancelled'
        ? `${runKind === 'search' ? 'search' : runKind} cancelled`
        : `stopped: ${STOPPED[str(to.stop_reason) ?? ''] ?? 'stopped'}`;
  const outcomes = (to.ops ?? {}) as Record<string, unknown>;
  return event(guide, what, 'outcome', `${state} · view only`, [
    { name: 'state', was: str(from.state) ?? '—', now: state },
    ...Object.keys(outcomes).map((outcome) => ({ name: `ops.${outcome}`, was: '—', now: String(num(outcomes[outcome])) })),
  ]);
}
