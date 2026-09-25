import { describe, expect, it } from 'vitest';
import { IDLE, onKey, cancel, fromMinutes, toMinutes, type LiftState, type MovableRun } from '../src/planner/keyboardMove';

const DAYS = ['Thu 24', 'Fri 25', 'Sat 26', 'Sun 27', 'Mon 28', 'Tue 29', 'Wed 30'];
const ctx = { dayLabel: (d: number) => DAYS[d]!, lastDay: 6 };
const run: MovableRun = { id: 'r-carling', day: 5, time: '22:00', label: 'HCarling + HStar' };

function press(state: LiftState, ...keys: string[]) {
  let current = state;
  let last = onKey(current, keys[0]!, run, ctx);
  for (const key of keys) {
    last = onKey(current, key, run, ctx);
    current = last.state;
  }
  return last;
}

describe('keyboard move reducer', () => {
  it('ignores arrows until the run is picked up', () => {
    expect(onKey(IDLE, 'ArrowLeft', run, ctx)).toEqual({ state: IDLE, handled: false });
  });

  it('leaves Enter and Space to open the card while idle', () => {
    for (const key of [' ', 'Enter']) expect(onKey(IDLE, key, run, ctx)).toEqual({ state: IDLE, handled: false });
  });

  it('picks up with M and explains the keys', () => {
    for (const key of ['m', 'M']) {
      const out = onKey(IDLE, key, run, ctx);
      expect(out.state).toEqual({ kind: 'lifted', runId: 'r-carling', origin: { day: 5, time: '22:00' }, at: { day: 5, time: '22:00' } });
      expect(out.announce).toMatch(/^Picked up HCarling \+ HStar, Tue 29, 22:00\. Left and right arrows/);
    }
  });

  it('moves by day and by 30 minutes, announcing each step', () => {
    const out = press(IDLE, 'm', 'ArrowRight');
    expect(out.announce).toBe('HCarling + HStar: Wed 30, 22:00.');
    const later = press(out.state, 'ArrowDown');
    expect(later.announce).toBe('HCarling + HStar: Wed 30, 22:30.');
    const earlier = press(later.state, 'ArrowUp', 'ArrowUp');
    expect(earlier.announce).toBe('HCarling + HStar: Wed 30, 21:30.');
  });

  it('stops at the edges of the week and the day', () => {
    const edge = press(IDLE, 'm', 'ArrowRight', 'ArrowRight');
    expect(edge.state).toMatchObject({ at: { day: 6 } });
    expect(edge.announce).toBe('Wed 30 is the last day of the boss week.');
    const late = press(IDLE, 'm', 'ArrowDown', 'ArrowDown', 'ArrowDown', 'ArrowDown');
    expect(late.state).toMatchObject({ at: { time: '23:30' } });
    expect(late.announce).toBe('23:30 is as late as this day goes.');
  });

  it('commits a changed slot on drop and nothing when unchanged', () => {
    const moved = press(IDLE, 'M', 'ArrowLeft', 'Enter');
    expect(moved.commit).toEqual({ runId: 'r-carling', from: { day: 5, time: '22:00' }, to: { day: 4, time: '22:00' } });
    expect(moved.state).toEqual(IDLE);
    expect(moved.announce).toBe('Dropped HCarling + HStar on Mon 28, 22:00.');

    const same = press(IDLE, 'm', 'ArrowLeft', 'ArrowRight', ' ');
    expect(same.commit).toBeUndefined();
    expect(same.announce).toBe('Dropped HCarling + HStar where it was. Nothing changed.');
  });

  it('cancels with Escape or by leaving the card, restoring the origin', () => {
    const esc = press(IDLE, 'm', 'ArrowLeft', 'Escape');
    expect(esc.state).toEqual(IDLE);
    expect(esc.commit).toBeUndefined();
    expect(esc.announce).toBe('Move cancelled. HCarling + HStar stays on Tue 29, 22:00.');

    const lifted = press(IDLE, 'm', 'ArrowLeft').state;
    expect(cancel(lifted, run, ctx).announce).toBe('Move cancelled. HCarling + HStar stays on Tue 29, 22:00.');
    expect(cancel(IDLE, run, ctx).handled).toBe(false);
  });

  it('keeps own-time runs timeless', () => {
    const own: MovableRun = { id: 'r-bellona', day: 4, time: null, label: 'NBellona' };
    const lifted = onKey(IDLE, 'm', own, ctx).state;
    const up = onKey(lifted, 'ArrowUp', own, ctx);
    expect(up.announce).toBe('Own-time runs have no time to change.');
    expect(onKey(lifted, 'ArrowRight', own, ctx).announce).toBe('NBellona: Tue 29, own time.');
  });

  it('round-trips clock arithmetic', () => {
    expect(toMinutes('21:30')).toBe(1290);
    expect(fromMinutes(1290)).toBe('21:30');
    expect(fromMinutes(0)).toBe('00:00');
  });
});
