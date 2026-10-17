import { describe, expect, it } from 'vitest';
import { namedIn, refusalWords, type KnownName } from '../src/members/tease';

const NAMES: KnownName[] = [
  { text: 'MikaBish', kind: 'IGN' },
  { text: 'ali', kind: 'alias' },
  { text: 'Ren', kind: 'name' },
];

describe('running-joke note refusals', () => {
  it('finds a known name only as a whole word, any case', () => {
    expect(namedIn('mikabish forgets her familiar', NAMES)?.name.text).toBe('MikaBish');
    expect(namedIn('forgot his alibi again', NAMES)).toBeNull();
    expect(namedIn('Ali dropped the pot', NAMES)?.name.kind).toBe('alias');
    expect(namedIn('rendered useless', NAMES)).toBeNull();
  });

  it('names the word and suggests the note without it', () => {
    expect(refusalWords('note_names_member', '', 'MikaBish forgets her familiar every single week', NAMES)).toBe(
      'Not saved: “MikaBish” is a roster IGN. A note never names anyone; try “forgets her familiar every single week”.',
    );
    expect(refusalWords('note_names_member', '', "Ren's pot again", NAMES)).toBe('Not saved: “Ren” is a roster name. A note never names anyone; try “pot again”.');
    expect(refusalWords('note_names_member', '', 'ali', NAMES)).toBe('Not saved: “ali” is an alias. A note never names anyone.');
  });

  it('says so plainly when the name is one the sheet does not know', () => {
    expect(refusalWords('note_names_member', '', 'RenShade carries again', [])).toBe(
      'Not saved: it names someone on the roster (a name, alias or IGN). A note never names anyone.',
    );
  });

  it('maps the other rules, and passes anything else through', () => {
    expect(refusalWords('note_too_long', '', 'x', NAMES)).toBe('Not saved: keep the note to 80 characters.');
    expect(refusalWords('note_not_plain', '', '@x', NAMES)).toBe('Not saved: plain text only, with no mentions, @ or # handles, links or formatting.');
    expect(refusalWords('invalid', 'No such member.', 'x', NAMES)).toBe('Not saved: No such member.');
  });
});
