import type { Channel, GuideRow } from '@kanade/api-types';
import type { SelectOption } from '@kanade/ui';

/**
 * The channel picker's options (approved default 3, G_EditorStates "Channel
 * picker open"): free channels first, this guide's own marked; channels
 * another guide uses are listed, disabled and named after that guide.
 */
export function channelOptions(channels: Channel[], guides: GuideRow[], own: string | null): SelectOption[] {
  const usedBy = new Map(guides.filter((g) => g.channel_id && g.id !== own).map((g) => [g.channel_id!, g.title]));
  const mine = guides.find((g) => g.id === own)?.channel_id ?? null;
  const free = channels
    .filter((c) => !usedBy.has(c.id))
    .map((c) => ({ value: c.id, label: c.name, group: 'Free', sub: c.id === mine ? 'this guide' : undefined }));
  const used = channels
    .filter((c) => usedBy.has(c.id))
    .map((c) => ({ value: c.id, label: c.name, group: 'Used by another guide', sub: usedBy.get(c.id), disabled: true }));
  return [...free, ...used];
}
