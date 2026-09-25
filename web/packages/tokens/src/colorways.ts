/** Appearance option keys; must match `_tokens.scss` selectors and theme-boot.js. */
export const COLORWAYS = [
  { key: 'marigold', name: 'Marigold', ground: '#eec75f', accent: '#4d5c9e' },
  { key: 'blossom', name: 'Blossom', ground: '#f2a8b8', accent: '#d5537a' },
  { key: 'periwinkle', name: 'Periwinkle', ground: '#9fb0e4', accent: '#4a5fae' },
  { key: 'coral', name: 'Coral', ground: '#e87d85', accent: '#d95965' },
  { key: 'twilight', name: 'Twilight', ground: '#8b7ad2', accent: '#6446ab' },
] as const;

export type Colorway = (typeof COLORWAYS)[number]['key'];
export const DEFAULT_COLORWAY: Colorway = 'marigold';

export const THEME_MODES = ['system', 'light', 'dark'] as const;
export type ThemeMode = (typeof THEME_MODES)[number];
