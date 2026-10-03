/** What the knowledge hero shows behind the boss's identity; decorative either way. */
export type HeroArt = { kind: 'video'; src: string; poster: string | undefined } | { kind: 'still'; src: string } | null;

export interface HeroArtInput {
  key: string;
  /** The looping MP4 URL from the API, or null where the deployment has none. */
  animated: string | null;
  reducedMotion: boolean;
  videoFailed: boolean;
  stillFailed: boolean;
}

/**
 * The animated art plays only when motion is welcome and it has not failed;
 * otherwise the still entry art stands in (it is also the video's poster).
 * Both failed: nothing, so the hero keeps its plain surface.
 */
export function heroArt({ key, animated, reducedMotion, videoFailed, stillFailed }: HeroArtInput): HeroArt {
  const still = stillFailed ? undefined : `/art/entry/${encodeURIComponent(key)}`;
  if (animated && !reducedMotion && !videoFailed) return { kind: 'video', src: animated, poster: still };
  return still ? { kind: 'still', src: still } : null;
}
