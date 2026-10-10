// Boss art in the PWAs (shell-and-components § "Boss art: animated where
// available"): the looping MP4 where the boss has one, else its still entry
// art. Discord never animates; nothing here reaches it.

/** What a boss-art slot shows; decorative either way. */
export type BossArtChoice = { kind: 'video'; src: string; poster: string | undefined } | { kind: 'still'; src: string } | null;

export interface BossArtInput {
  /** The still entry art URL (`/art/entry/{key}`), or null where there is none. */
  still: string | null;
  /** The looping MP4 URL from the API, or null where the deployment has none. */
  animated: string | null;
  reducedMotion: boolean;
  videoFailed: boolean;
  stillFailed: boolean;
}

/** The still entry art's path for a boss key (the knowledge hero has no `art` URL to read). */
export function entryArt(key: string): string {
  return `/art/entry/${encodeURIComponent(key)}`;
}

/**
 * The animated art plays only when motion is welcome and it has not failed;
 * otherwise the still entry art stands in (it is also the video's poster).
 * Both failed: nothing, so the slot keeps its plain surface.
 */
export function bossArt({ still, animated, reducedMotion, videoFailed, stillFailed }: BossArtInput): BossArtChoice {
  const image = stillFailed || !still ? undefined : still;
  if (animated && !reducedMotion && !videoFailed) return { kind: 'video', src: animated, poster: image };
  return image ? { kind: 'still', src: image } : null;
}
