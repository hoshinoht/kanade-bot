import type { Boss, EventBoss } from '@kanade/api-types';

/** The label event bosses wear: their Challengers World season is the event's name. */
export function seasonal(event: { name: string }): string {
  return `Seasonal boss · ${event.name}`;
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
    hue: 0,
  };
}
