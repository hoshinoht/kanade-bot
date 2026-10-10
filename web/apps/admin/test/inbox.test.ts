import { describe, expect, it } from 'vitest';
import type { Proposal } from '@kanade/api-types';
import { editable, parseEdit } from '../src/inbox/edit';
import { blocked, reasonProblem, refusalText } from '../src/inbox/flags';

const proposal = (over: Partial<Proposal>): Proposal =>
  ({
    id: 'p',
    short_id: 'abc',
    kind: 'move',
    kind_label: 'Move',
    source: 'extraction',
    tab: 'extractor',
    version: 1,
    flags: [],
    preview: { no_effect: false, changes: [], conflicts: [] },
    choices: null,
    when: 'Wed 30 Sep 23:30',
    ...over,
  }) as Proposal;

describe('edit, then approve', () => {
  it('counts the day in the proposed time’s boss week and always sends HH:MM', () => {
    // Thursday resets: Wed is day 6 of the proposal's week, whichever week is on screen.
    expect(parseEdit('22:30', proposal({}), 'Thu')).toEqual({ ok: true, day: 6, time: '22:30' });
    expect(parseEdit('fri 9pm', proposal({}), 'Thu')).toEqual({ ok: true, day: 1, time: '21:00' });
    expect(parseEdit('thu', proposal({ when: 'Mon 05 Oct 20:00' }), 'Thu')).toEqual({ ok: true, day: 0, time: '20:00' });
    expect(parseEdit('soon', proposal({}), 'Thu')).toMatchObject({ ok: false });
  });

  it('is offered for a move, a new run or a split from Kanade only', () => {
    expect(editable(proposal({}))).toBe(true);
    expect(editable(proposal({ kind: 'add' }))).toBe(true);
    expect(editable(proposal({ kind: 'cancel' }))).toBe(false);
    expect(editable(proposal({ kind: 'join', tab: 'self_service', source: 'self_service' }))).toBe(false);
    expect(editable(proposal({ flags: ['expired'] }))).toBe(false);
  });
});

describe('inbox rules in words', () => {
  it('lets conflicts and an unauthorised requester block approval', () => {
    expect(blocked(proposal({ preview: { no_effect: false, changes: [], conflicts: [{ field: 'slot', expected: 'a', found: 'b' }] } }))).toContain('reject it');
    expect(blocked(proposal({ flags: ['requester_unauthorised'] }))).toContain('no longer have this approved');
    expect(blocked(proposal({}))).toBe('');
  });

  it('asks a reason of member requests only', () => {
    expect(reasonProblem(proposal({}), '')).toBe('');
    expect(reasonProblem(proposal({ tab: 'self_service', source: 'self_service' }), '')).toContain('Say why');
  });

  it('has plain words for the new refusal codes', () => {
    for (const code of ['discord_session_required', 'edit_not_applicable', 'reason_not_applicable', 'requester_unauthorised', 'version_required', 'force_unsupported']) {
      expect(refusalText(code, 'server words')).not.toBe('server words');
    }
    expect(refusalText('busy', 'Another change landed; try again.')).toBe('Another change landed; try again.');
  });
});
