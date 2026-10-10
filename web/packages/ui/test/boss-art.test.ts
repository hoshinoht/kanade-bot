import { describe, expect, it } from 'vitest';
import { bossArt, entryArt, type BossArtInput } from '../src/bossArt';

const base: BossArtInput = { still: '/art/entry/MaleficStar', animated: '/art/animated/MaleficStar', reducedMotion: false, videoFailed: false, stillFailed: false };

describe('boss art', () => {
  it('plays the animated art with the still entry art as poster', () => {
    expect(bossArt(base)).toEqual({ kind: 'video', src: '/art/animated/MaleficStar', poster: '/art/entry/MaleficStar' });
  });

  it('shows the still under reduced motion, without animated art, or after the video fails', () => {
    const still = { kind: 'still', src: '/art/entry/MaleficStar' };
    expect(bossArt({ ...base, reducedMotion: true })).toEqual(still);
    expect(bossArt({ ...base, animated: null })).toEqual(still);
    expect(bossArt({ ...base, videoFailed: true })).toEqual(still);
  });

  it('keeps a playable video without a poster when the still failed or is absent', () => {
    const bare = { kind: 'video', src: '/art/animated/MaleficStar', poster: undefined };
    expect(bossArt({ ...base, stillFailed: true })).toEqual(bare);
    expect(bossArt({ ...base, still: null })).toEqual(bare);
  });

  it('shows nothing when neither art is available', () => {
    expect(bossArt({ ...base, stillFailed: true, videoFailed: true })).toBeNull();
    expect(bossArt({ ...base, animated: null, stillFailed: true })).toBeNull();
    expect(bossArt({ ...base, reducedMotion: true, stillFailed: true })).toBeNull();
    expect(bossArt({ ...base, still: null, animated: null })).toBeNull();
  });

  it('encodes the key into the still path', () => {
    expect(entryArt('MaleficStar')).toBe('/art/entry/MaleficStar');
    expect(entryArt('A B')).toBe('/art/entry/A%20B');
  });
});
