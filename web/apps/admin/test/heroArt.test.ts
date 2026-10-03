import { describe, expect, it } from 'vitest';
import { heroArt, type HeroArtInput } from '../src/bosses/heroArt';

const base: HeroArtInput = { key: 'MaleficStar', animated: '/art/animated/MaleficStar', reducedMotion: false, videoFailed: false, stillFailed: false };

describe('knowledge hero art', () => {
  it('plays the animated art with the still entry art as poster', () => {
    expect(heroArt(base)).toEqual({ kind: 'video', src: '/art/animated/MaleficStar', poster: '/art/entry/MaleficStar' });
  });

  it('shows the still under reduced motion, without animated art, or after the video fails', () => {
    const still = { kind: 'still', src: '/art/entry/MaleficStar' };
    expect(heroArt({ ...base, reducedMotion: true })).toEqual(still);
    expect(heroArt({ ...base, animated: null })).toEqual(still);
    expect(heroArt({ ...base, videoFailed: true })).toEqual(still);
  });

  it('keeps a playable video without a poster when only the still failed', () => {
    expect(heroArt({ ...base, stillFailed: true })).toEqual({ kind: 'video', src: '/art/animated/MaleficStar', poster: undefined });
  });

  it('shows nothing when neither art is available', () => {
    expect(heroArt({ ...base, stillFailed: true, videoFailed: true })).toBeNull();
    expect(heroArt({ ...base, animated: null, stillFailed: true })).toBeNull();
    expect(heroArt({ ...base, reducedMotion: true, stillFailed: true })).toBeNull();
  });

  it('encodes the key into the still path', () => {
    expect(heroArt({ ...base, key: 'A B', animated: null })).toEqual({ kind: 'still', src: '/art/entry/A%20B' });
  });
});
