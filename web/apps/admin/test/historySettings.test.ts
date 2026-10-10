import { describe, expect, it } from 'vitest';
import type { ChangeRecord, SettingsChangeRow } from '@kanade/api-types';
import { mergeTimeline, sectionHref, settingCount, settingFields, settingSummary, windowClear } from '../src/history/settings';

const save = (over: Partial<SettingsChangeRow>): SettingsChangeRow => ({
  id: 1,
  at: '2026-09-29T04:00:00+00:00',
  actor: { kind: 'admin', id: 'token' },
  surface: 'admin_portal',
  section: 'notifications',
  revision: 1,
  week: '2026-09-23T16:00:00+00:00',
  values: [{ key: 'quiet_mode', from: '0', to: '1' }],
  ...over,
});
const record = (seq: number, at: string) => ({ seq, at }) as ChangeRecord;

describe('Config saves on the History timeline', () => {
  it('summarises the section and its first keys', () => {
    expect(settingSummary(save({}))).toBe('Config · Notifications — quiet_mode');
    const many = save({
      section: 'persona',
      values: ['persona', 'v5.profile_visibility', 'v5.role_profiles', 'chat_mode'].map((key) => ({ key, from: '', to: 'x' })),
    });
    expect(settingSummary(many)).toBe('Config · Persona — persona, profile_visibility, role_profiles +1 more');
  });

  it('links to the Config section key the page uses', () => {
    expect(sectionHref('run_lengths')).toBe('/config?section=run-lengths');
    expect(sectionHref('self_service')).toBe('/config?section=self-service');
    expect(sectionHref('persona')).toBe('/config?section=persona');
  });

  it('lists plain rows as they are and JSON rows field by field', () => {
    const fields = settingFields(
      save({
        values: [
          { key: 'quiet_mode', from: '0', to: '1' },
          { key: 'persona', from: '', to: 'calm' },
          {
            key: 'v5.profanity',
            from: '{"extra_words":[],"check_questions":true}',
            to: '{"extra_words":["frick"],"check_questions":true}',
          },
        ],
      }),
    );
    expect(fields).toEqual([
      { name: 'quiet_mode', was: '0', now: '1' },
      { name: 'persona', was: '—', now: 'calm' },
      { name: 'profanity.extra_words', was: '[]', now: '["frick"]' },
    ]);
  });

  it('places each save above the first record at or before it', () => {
    const records = [record(4, '2026-09-29T05:00:00+00:00'), record(3, '2026-09-29T03:00:00+00:00'), record(2, '2026-09-28T00:00:00+00:00')];
    const settings = [save({ id: 1, at: '2026-09-27T00:00:00+00:00' }), save({ id: 2, at: '2026-09-29T03:00:00+00:00' }), save({ id: 3, at: '2026-09-29T06:00:00+00:00' })];
    const order = mergeTimeline(records, settings).map((item) => (item.kind === 'change' ? `#${item.record.seq}` : `c${item.change.id}`));
    expect(order).toEqual(['c3', '#4', 'c2', '#3', '#2', 'c1']);
  });
});

describe('Limits window clears on the History timeline', () => {
  const clear = save({
    section: 'limits',
    revision: 0,
    values: [
      {
        key: 'window.1003',
        from: '{"member":"Mika","used":3,"limit":4,"per_s":300,"overridden":false}',
        to: '{"member":"Mika","used":0,"limit":4,"per_s":300,"overridden":false}',
      },
    ],
  });

  it('reads the window as it was and summarises the clear', () => {
    expect(windowClear(clear)).toEqual({ memberId: '1003', member: 'Mika', used: 3, limit: 4, perS: 300, overridden: false });
    expect(windowClear(save({}))).toBeNull();
    expect(settingSummary(clear)).toBe("Limits · cleared Mika's chat window (3 used)");
    expect(settingCount(clear)).toBe('chat window');
    expect(sectionHref('limits')).toBe('/limits');
  });

  it('lists only the count that changed', () => {
    expect(settingFields(clear)).toEqual([{ name: 'window.1003.used', was: '3', now: '0' }]);
  });
});
