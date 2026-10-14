import {
  COLORWAY_GROUPS,
  COLORWAYS,
  DEFAULT_COLORWAY,
  DYNAMIC_CACHE_KEY,
  THEME_MODES,
  type Colorway,
  type ThemeMode,
} from '@kanade/tokens/colorways';
import { dynamicPalette, dynamicProperties } from '@kanade/tokens/dynamic';

const COLORWAY_KEY = 'colorway';
const THEME_KEY = 'theme';
/** Same origin on both apps, so the canvas stays readable under img-src 'self'. */
const AVATAR = '/identity/avatar';

function store(key: string, value: string | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Storage can be unavailable; the choice then lasts for this page only.
  }
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function currentColorway(): Colorway {
  const value = document.documentElement.dataset.colorway ?? read(COLORWAY_KEY);
  return (COLORWAYS.find((way) => way.key === value)?.key ?? DEFAULT_COLORWAY) as Colorway;
}

export function currentMode(): ThemeMode {
  const value = document.documentElement.dataset.theme;
  return value === 'light' || value === 'dark' ? value : 'system';
}

export function applyColorway(way: Colorway): void {
  document.documentElement.dataset.colorway = way;
  store(COLORWAY_KEY, way);
  if (way === 'dynamic') void refreshDynamic();
}

export function applyMode(mode: ThemeMode): void {
  if (!THEME_MODES.includes(mode)) return;
  if (mode === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = mode;
  store(THEME_KEY, mode === 'system' ? null : mode);
}

/** The set a colourway belongs to. */
export function setOf(way: Colorway): string {
  return COLORWAYS.find((w) => w.key === way)?.set ?? 'base';
}

// Which picker sets are expanded: memory only, for this page's life (never
// stored). Seeded once with the set holding the current colourway.
let openSets: Set<string> | null = null;

/** The expanded sets, as a fresh record for a picker's own state. */
export function openColorwaySets(): Record<string, boolean> {
  openSets ??= new Set([setOf(currentColorway())]);
  return Object.fromEntries([...openSets].map((key) => [key, true]));
}

/** Remember a set's expanded state for the next picker this session. */
export function rememberColorwaySet(key: string, open: boolean): void {
  openSets ??= new Set([setOf(currentColorway())]);
  if (open) openSets.add(key);
  else openSets.delete(key);
}

function clearDynamic(root: HTMLElement): void {
  for (const name of Array.from(root.style)) if (name.startsWith('--dyn-')) root.style.removeProperty(name);
  store(DYNAMIC_CACHE_KEY, null);
}

let refreshing: Promise<void> | null = null;

/**
 * Derive the Dynamic palette from the bot's avatar and set it as `--dyn-*`
 * properties on <html> (the pickers preview it whichever colourway is on).
 * A palette that fails its contrast checks, or an avatar that will not load,
 * clears them, and `_tokens.scss` falls back to marigold. `data-dynamic`
 * says which happened.
 */
export function refreshDynamic(): Promise<void> {
  refreshing ??= (async () => {
    const root = document.documentElement;
    try {
      const img = new Image();
      img.decoding = 'async';
      img.src = AVATAR;
      await img.decode();
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 48;
      const ctx = canvas.getContext('2d', { willReadFrequently: true });
      if (!ctx) throw new Error('no 2d context');
      ctx.drawImage(img, 0, 0, 48, 48);
      const palette = dynamicPalette(ctx.getImageData(0, 0, 48, 48).data);
      if (!palette) throw new Error('palette refused');
      const props = dynamicProperties(palette);
      for (const [name, value] of props) root.style.setProperty(name, value);
      store(DYNAMIC_CACHE_KEY, JSON.stringify(Object.fromEntries(props)));
      root.dataset.dynamic = 'avatar';
    } catch {
      clearDynamic(root);
      root.dataset.dynamic = 'fallback';
    } finally {
      refreshing = null;
    }
  })();
  return refreshing;
}

// A stored Dynamic choice: theme-boot painted the cached palette; check it against today's avatar.
if (typeof document !== 'undefined' && document.documentElement.dataset.colorway === 'dynamic') void refreshDynamic();

export { COLORWAY_GROUPS, COLORWAYS, THEME_MODES };
export type { Colorway, ThemeMode };
