/**
 * Every v4 portal section as a v5 admin route, in v4's nav order and groups.
 * Every page is built; docs/v5/pwa-parity.md tracks what each still lacks.
 */

export type Group = 'Schedule' | 'Kanade' | 'Operate';

export interface Section {
  key: string;
  href: string;
  label: string;
  group: Group;
  title: string;
}

export const SECTIONS: Section[] = [
  { key: 'week', href: '/', label: 'Week', group: 'Schedule', title: 'Week' },
  {
    key: 'fixed',
    href: '/fixed',
    label: 'Fixed',
    group: 'Schedule',
    title: 'Weekly timings',
  },
  {
    key: 'bosses',
    href: '/bosses',
    label: 'Bosses',
    group: 'Schedule',
    title: 'Bosses',
  },
  {
    key: 'inbox',
    href: '/inbox',
    label: 'Inbox',
    group: 'Kanade',
    title: 'Inbox',
  },
  {
    key: 'extractions',
    href: '/extractions',
    label: 'Extractions',
    group: 'Kanade',
    title: 'Extractions',
  },
  {
    key: 'chat',
    href: '/chat',
    label: 'Chat',
    group: 'Kanade',
    title: 'Chat',
  },
  {
    key: 'limits',
    href: '/limits',
    label: 'Limits',
    group: 'Kanade',
    title: 'Limits',
  },
  {
    key: 'members',
    href: '/members',
    label: 'Members',
    group: 'Operate',
    title: 'Members',
  },
  {
    key: 'reminders',
    href: '/reminders',
    label: 'Reminders',
    group: 'Operate',
    title: 'Reminders',
  },
  { key: 'config', href: '/config', label: 'Config', group: 'Operate', title: 'Config' },
  // v4 "Audit", rebuilt on the git-style change history; /audit redirects here.
  { key: 'history', href: '/history', label: 'History', group: 'Operate', title: 'History' },
];

/** Detail pages reached from a section; they share its nav highlight. */
export const DETAILS: { key: string; pattern: string; section: string; title: string }[] = [
  {
    key: 'boss-knowledge',
    pattern: '/bosses/:boss/knowledge',
    section: 'bosses',
    title: 'Boss knowledge',
  },
  {
    key: 'extraction',
    pattern: '/extractions/:id',
    section: 'extractions',
    title: 'Extraction',
  },
  {
    key: 'chat-interaction',
    pattern: '/chat/:id',
    section: 'chat',
    title: 'Chat interaction',
  },
];

export const GROUPS: Group[] = ['Schedule', 'Kanade', 'Operate'];
/** Kept in reach on a phone; the rest fold into "More" (v4 nav_pinned). */
export const PINNED = ['week', 'inbox'];

export const ROUTES = [
  { key: 'login', pattern: '/login' },
  ...SECTIONS.map((s) => ({ key: s.key, pattern: s.href })),
  ...DETAILS.map((d) => ({ key: d.key, pattern: d.pattern })),
];
