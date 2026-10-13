// APG menu button keys, shared by AccountMenu, the admin rail's More and the
// roster rows' More: items are the menu's `role="menuitem"` elements.

const itemsOf = (menu: HTMLElement | undefined) => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

/** Focuses the open menu's first or last item. */
export function focusMenuItem(menu: HTMLElement | undefined, at: 'first' | 'last') {
  const all = itemsOf(menu);
  (at === 'first' ? all[0] : all[all.length - 1])?.focus();
}

/** The menu button's keys: ↓/Enter/Space open on the first item, ↑ on the last. */
export function menuButtonKey(event: KeyboardEvent, show: (at: 'first' | 'last') => void) {
  if (event.key === 'ArrowDown' || event.key === 'Enter' || event.key === ' ') {
    event.preventDefault();
    show('first');
  } else if (event.key === 'ArrowUp') {
    event.preventDefault();
    show('last');
  }
}

/** The open menu's keys: arrows move and wrap, Home/End jump, Escape closes back to the button, Tab closes. */
export function menuKey(event: KeyboardEvent, menu: HTMLElement | undefined, hide: (refocus?: boolean) => void) {
  const all = itemsOf(menu);
  const at = all.indexOf(document.activeElement as HTMLElement);
  const moves: Record<string, number> = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: all.length - 1 };
  if (event.key in moves) {
    event.preventDefault();
    all[(moves[event.key]! + all.length) % all.length]?.focus();
  } else if (event.key === 'Escape') {
    event.preventDefault();
    hide();
  } else if (event.key === 'Tab') {
    hide(false);
  }
}
