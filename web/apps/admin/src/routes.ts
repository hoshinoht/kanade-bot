/**
 * Every v4 portal section as a v5 admin route, in v4's nav order and groups.
 * Every page is built; docs/notes/pwa-parity.md tracks what each still lacks.
 */

import type { IconName } from '@kanade/ui';

export type Group = 'Schedule' | 'Kanade' | 'Operate';

export interface Section {
  key: string;
  href: string;
  label: string;
  group: Group;
  title: string;
  /** The navigation rail's glyph; the label is always shown beside or under it. */
  icon: IconName;
}

export const SECTIONS: Section[] = [
  { key: 'week', href: '/', label: 'Week', group: 'Schedule', title: 'Week', icon: 'calendar' },
  {
    key: 'fixed',
    href: '/fixed',
    label: 'Fixed',
    group: 'Schedule',
    title: 'Weekly timings',
    icon: 'pin',
  },
  {
    key: 'bosses',
    href: '/bosses',
    label: 'Bosses',
    group: 'Schedule',
    title: 'Bosses',
    icon: 'shield',
  },
  {
    key: 'inbox',
    href: '/inbox',
    label: 'Inbox',
    group: 'Kanade',
    title: 'Inbox',
    icon: 'inbox',
  },
  {
    key: 'extractions',
    href: '/extractions',
    label: 'Extractions',
    group: 'Kanade',
    title: 'Extractions',
    icon: 'filter',
  },
  {
    key: 'chat',
    href: '/chat',
    label: 'Chat',
    group: 'Kanade',
    title: 'Chat',
    icon: 'message-square',
  },
  {
    key: 'limits',
    href: '/limits',
    label: 'Limits',
    group: 'Kanade',
    title: 'Limits',
    icon: 'gauge',
  },
  {
    key: 'members',
    href: '/members',
    label: 'Members',
    group: 'Operate',
    title: 'Members',
    icon: 'users',
  },
  {
    key: 'reminders',
    href: '/reminders',
    label: 'Reminders',
    group: 'Operate',
    title: 'Reminders',
    icon: 'bell',
  },
  { key: 'config', href: '/config', label: 'Config', group: 'Operate', title: 'Config', icon: 'sliders' },
  // v4 "Audit", rebuilt on the git-style change history; /audit redirects here.
  { key: 'history', href: '/history', label: 'History', group: 'Operate', title: 'History', icon: 'history' },
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

export const ROUTES = [
  { key: 'login', pattern: '/login' },
  // Reached from the account menu; no navigation destination of its own.
  { key: 'account', pattern: '/account' },
  ...SECTIONS.map((s) => ({ key: s.key, pattern: s.href })),
  ...DETAILS.map((d) => ({ key: d.key, pattern: d.pattern })),
];
