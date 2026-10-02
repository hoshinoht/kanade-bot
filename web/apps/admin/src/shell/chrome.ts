import type { FreshState } from '@kanade/ui';
import { getContext, setContext } from 'svelte';

/** What the page line needs from the shell: data status and the palette (M3E spec "Frame"). */
export interface Chrome {
  /** Phone frame (top bar + drawer): the top bar carries the title and the Live chip instead. */
  readonly phone: boolean;
  readonly fresh: FreshState;
  readonly updated: string;
  readonly timezone: string;
  palette(): void;
  /**
   * Phone frame: a page's own back step in the top bar (an open Inbox item:
   * "‹ Inbox"), shown in place of the menu and title; `null` clears it.
   */
  back(step: BackStep | null): void;
}

/** A back step for the top bar: its visible label and accessible name. */
export interface BackStep {
  readonly label: string;
  readonly name: string;
  go(): void;
}

const KEY = Symbol('kanade-chrome');

export const setChrome = (chrome: Chrome) => setContext(KEY, chrome);
export const getChrome = () => getContext<Chrome | undefined>(KEY);

/**
 * The phone frame: narrow screens, and phone landscape (a 96 px rail of eleven
 * destinations does not fit under 500 px of height).
 */
export const PHONE_QUERY = '(max-width: 599px), (max-height: 500px)';

const RAIL_KEY = 'rail';

/** The expanded rail (≥ 1440 px) can be collapsed; the choice is remembered in this browser. */
export function railCollapsed(): boolean {
  try {
    return localStorage.getItem(RAIL_KEY) === 'collapsed';
  } catch {
    return false;
  }
}

export function rememberRail(collapsed: boolean): void {
  try {
    if (collapsed) localStorage.setItem(RAIL_KEY, 'collapsed');
    else localStorage.removeItem(RAIL_KEY);
  } catch {
    /* storage blocked: the choice lasts for this page only */
  }
}
