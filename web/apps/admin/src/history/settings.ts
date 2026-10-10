/**
 * Config section saves on the History timeline (`HistoryPage.settings`):
 * view-only rows outside the hash chain. Values are the stored `config` row
 * text, so JSON rows (run lengths, profanity, context) are compared field by
 * field and plain rows as they are.
 */
import type { ChangeRecord, SettingsChangeRow } from '@kanade/api-types';

/** API section → its Config label and `/config?section=` key. */
const SECTIONS: Record<string, { label: string; key: string }> = {
  pings: { label: 'Pings', key: 'pings' },
  watching: { label: 'Chat watching', key: 'watching' },
  chatbot: { label: 'Chatbot', key: 'chatbot' },
  notifications: { label: 'Notifications', key: 'notifications' },
  self_service: { label: 'Self-service', key: 'self-service' },
  persona: { label: 'Persona', key: 'persona' },
  models: { label: 'Models', key: 'models' },
  run_lengths: { label: 'Run lengths', key: 'run-lengths' },
  profanity: { label: 'Profanity', key: 'profanity' },
};

/** A cleared Limits chat window, recorded beside the saves (section `limits`). */
export const LIMITS_SECTION = 'limits';
export const isWindowClear = (change: SettingsChangeRow) => change.section === LIMITS_SECTION;

export const sectionLabel = (section: string) => (section === LIMITS_SECTION ? 'Limits' : (SECTIONS[section]?.label ?? section));
export const sectionHref = (section: string) =>
  section === LIMITS_SECTION ? '/limits' : `/config?section=${encodeURIComponent(SECTIONS[section]?.key ?? section)}`;
/** A row key without the `v5.` storage prefix. */
const keyLabel = (key: string) => key.replace(/^v5\./, '');

export const settingCount = (change: SettingsChangeRow) =>
  isWindowClear(change) ? 'chat window' : `${change.values.length} setting${change.values.length === 1 ? '' : 's'}`;

export interface WindowClear {
  memberId: string;
  member: string;
  /** Answers in the window when it was cleared. */
  used: number;
  limit: number;
  perS: number;
  overridden: boolean;
}

/** The window a `limits` record cleared, as it was; `null` for a Config save. */
export function windowClear(change: SettingsChangeRow): WindowClear | null {
  const row = change.values[0];
  if (!isWindowClear(change) || !row) return null;
  const memberId = row.key.replace(/^window\./, '');
  const before = object(row.from) ?? {};
  const number = (value: unknown) => (typeof value === 'number' ? value : 0);
  return {
    memberId,
    member: typeof before.member === 'string' ? before.member : memberId,
    used: number(before.used),
    limit: number(before.limit),
    perS: number(before.per_s),
    overridden: before.overridden === true,
  };
}

/** "Config · Persona — persona, chat_mode": the section and the first keys it changed. */
export function settingSummary(change: SettingsChangeRow): string {
  const clear = windowClear(change);
  if (clear) return `Limits · cleared ${clear.member}'s chat window (${clear.used} used)`;
  const keys = change.values.map((row) => keyLabel(row.key));
  const shown = keys.slice(0, 3).join(', ');
  const more = keys.length > 3 ? ` +${keys.length - 3} more` : '';
  return `Config · ${sectionLabel(change.section)} — ${shown}${more}`;
}

const object = (text: string): Record<string, unknown> | null => {
  try {
    const value: unknown = JSON.parse(text);
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
};
const shown = (value: unknown) => (value === undefined || value === '' ? '—' : typeof value === 'string' ? value : JSON.stringify(value));

export interface SettingField {
  name: string;
  was: string;
  now: string;
}

/** One line per changed setting; a JSON row lists only its fields that differ. */
export function settingFields(change: SettingsChangeRow): SettingField[] {
  return change.values.flatMap((row) => {
    const before = object(row.from);
    const after = object(row.to);
    if (!before || !after) return [{ name: keyLabel(row.key), was: shown(row.from), now: shown(row.to) }];
    const names = [...new Set([...Object.keys(before), ...Object.keys(after)])];
    const fields = names
      .filter((name) => JSON.stringify(before[name]) !== JSON.stringify(after[name]))
      .map((name) => ({ name: `${keyLabel(row.key)}.${name}`, was: shown(before[name]), now: shown(after[name]) }));
    return fields.length ? fields : [{ name: keyLabel(row.key), was: shown(row.from), now: shown(row.to) }];
  });
}

export type TimelineItem = { kind: 'change'; record: ChangeRecord } | { kind: 'config'; change: SettingsChangeRow };

/**
 * Records keep their page order (newest seq first); each save goes just
 * above the first record at or before its time, else after them all.
 */
export function mergeTimeline(records: ChangeRecord[], settings: SettingsChangeRow[]): TimelineItem[] {
  const saves = [...settings].sort((a, b) => Date.parse(b.at) - Date.parse(a.at) || b.id - a.id);
  const items: TimelineItem[] = [];
  let next = 0;
  for (const record of records) {
    const at = Date.parse(record.at);
    while (next < saves.length && Date.parse(saves[next]!.at) >= at) items.push({ kind: 'config', change: saves[next++]! });
    items.push({ kind: 'change', record });
  }
  while (next < saves.length) items.push({ kind: 'config', change: saves[next++]! });
  return items;
}
