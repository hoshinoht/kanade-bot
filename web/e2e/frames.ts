import { ADMIN, PUBLIC } from './support';

/** The five layout frames every screen is checked at (`layout.spec`, `clipping.spec`). */
export const SIZES = [
  { width: 1280, height: 800 },
  { width: 1000, height: 670 },
  { width: 1280, height: 600 },
  { width: 390, height: 844 },
  { width: 844, height: 390 },
] as const;

/** Every admin and public screen, as [app, origin, path]. */
export const SCREENS: [string, string, string][] = [
  ['admin', ADMIN, '/'],
  ['admin', ADMIN, '/fixed'],
  ['admin', ADMIN, '/bosses'],
  ['admin', ADMIN, '/bosses/Carling/knowledge'],
  ['admin', ADMIN, '/members'],
  ['admin', ADMIN, '/reminders'],
  ['admin', ADMIN, '/inbox'],
  ['admin', ADMIN, '/extractions'],
  ['admin', ADMIN, '/extractions/x-kalos'],
  ['admin', ADMIN, '/chat'],
  ['admin', ADMIN, '/chat/c-move'],
  ['admin', ADMIN, '/chat/c-when'],
  ['admin', ADMIN, '/chat/c-safe-line'],
  ['admin', ADMIN, '/limits'],
  ['admin', ADMIN, '/account'],
  ['admin', ADMIN, '/history'],
  ['admin', ADMIN, '/config'],
  ['admin', ADMIN, '/config?section=models'],
  ['admin', ADMIN, '/config?section=persona'],
  ['admin', ADMIN, '/config?section=profanity'],
  ['public', PUBLIC, '/'],
];

/** The URL a screen is opened at, with the service worker off. */
export const screenUrl = (origin: string, path: string) => `${origin}${path}${path.includes('?') ? '&' : '?'}sw=off`;
