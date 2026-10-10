import type { Boss, EventBoss } from '@kanade/api-types';

/**
 * The short tag an event boss wears: "Challengers World Season 3" becomes
 * "CW3"; any other event name is kept whole. The full name stays in the data
 * (and the tag's `title`).
 */
export function seasonTag(event: { name: string }): string {
  const season = /^Challengers World Season (\d+)$/i.exec(event.name.trim());
  return season ? `CW${season[1]}` : event.name;
}

/** `Portrait` wants a Boss; an event boss shows its portrait, else its icon. */
export function eventAsBoss(boss: EventBoss): Boss {
  return {
    token: boss.key,
    key: boss.key,
    name: boss.key,
    difficulty: 'n',
    level: null,
    portrait: boss.portrait ?? boss.portrait_sm,
    portrait_sm: boss.portrait_sm,
    art: boss.art,
    animated: boss.animated,
    hue: 0,
  };
}
