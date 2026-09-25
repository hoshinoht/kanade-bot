import { COLORWAYS, DEFAULT_COLORWAY, THEME_MODES, type Colorway, type ThemeMode } from '@kanade/tokens/colorways';

const COLORWAY_KEY = 'colorway';
const THEME_KEY = 'theme';

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
}

export function applyMode(mode: ThemeMode): void {
  if (!THEME_MODES.includes(mode)) return;
  if (mode === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = mode;
  store(THEME_KEY, mode === 'system' ? null : mode);
}

export { COLORWAYS, THEME_MODES };
export type { Colorway, ThemeMode };
