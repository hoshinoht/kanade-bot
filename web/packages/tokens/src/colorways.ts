/** Colourway sets, in picker order. `note` is a small line under the set's label. */
export const COLORWAY_SETS = [
  { key: 'base', name: 'Base' },
  { key: 'blue-archive', name: 'Blue Archive', note: 'Colourways inspired by Blue Archive' },
  { key: 'terminal', name: 'Terminal', note: 'Inspired by Catppuccin, Tokyo Night and GitHub themes' },
  { key: 'dynamic', name: 'Dynamic', note: "From the bot's avatar" },
] as const;

export type ColorwaySet = (typeof COLORWAY_SETS)[number]['key'];

/**
 * Appearance option keys; must match `_tokens.scss` selectors and theme-boot.js.
 * The four base keys predate their character names and stay as stored values.
 * `ground`/`accent` are each light face's anchors (dynamic: its marigold fallback).
 */
export const COLORWAYS = [
  { key: 'marigold', name: 'Otonose', set: 'base', ground: '#eec75f', accent: '#4d5c9e' },
  { key: 'blossom', name: 'Nazuna', set: 'base', ground: '#f2a8b8', accent: '#d5537a' },
  { key: 'periwinkle', name: 'Sumire', set: 'base', ground: '#9fb0e4', accent: '#4a5fae' },
  { key: 'twilight', name: 'Hinano', set: 'base', ground: '#8b7ad2', accent: '#6446ab' },
  { key: 'hoshino', name: 'Hoshino', set: 'blue-archive', ground: '#f3bdca', accent: '#095366' },
  { key: 'mika', name: 'Mika', set: 'blue-archive', ground: '#eaa6d2', accent: '#a0306d' },
  { key: 'seia', name: 'Seia', set: 'blue-archive', ground: '#f0c9a8', accent: '#a8456a' },
  { key: 'hina', name: 'Hina', set: 'blue-archive', ground: '#cfcbdb', accent: '#6b2f86' },
  { key: 'aris', name: 'Aris', set: 'blue-archive', ground: '#6cc4e4', accent: '#0d62a6' },
  { key: 'catppuccin', name: 'Catppuccin', set: 'terminal', ground: '#dce0e8', accent: '#8839ef' },
  { key: 'tokyonight', name: 'Tokyo Night', set: 'terminal', ground: '#c1c9df', accent: '#2e7de9' },
  { key: 'github', name: 'GitHub', set: 'terminal', ground: '#f6f8fa', accent: '#0969da' },
  { key: 'dynamic', name: 'Avatar', set: 'dynamic', ground: '#eec75f', accent: '#4d5c9e' },
] as const;

export type Colorway = (typeof COLORWAYS)[number]['key'];
export const DEFAULT_COLORWAY: Colorway = 'marigold';

/** The colourways grouped by set, for the pickers. */
export const COLORWAY_GROUPS = COLORWAY_SETS.map((set) => ({
  ...set,
  ways: COLORWAYS.filter((way) => way.set === set.key),
}));

/** localStorage key of the Dynamic palette derived from the avatar (a cache, not a choice). */
export const DYNAMIC_CACHE_KEY = 'colorway-dynamic';

export const THEME_MODES = ['system', 'light', 'dark'] as const;
export type ThemeMode = (typeof THEME_MODES)[number];
