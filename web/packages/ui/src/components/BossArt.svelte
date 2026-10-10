<!--
  A boss's entry art as a drop-in for its `<img>` (shell-and-components § "Boss
  art: animated where available"): the looping MP4 where the boss has one, muted
  and decorative with the still as its poster, else the still. The caller's
  class owns crop, veil, mask and stacking, so either element lands in the same
  box. The still stands in under reduced motion (device or the admin switch) and
  after the video fails; a video off screen is paused so a full board does not
  decode every clip at once. Discord never animates: this is for the PWAs only.
-->
<script lang="ts">
  import type { Attachment } from 'svelte/attachments';
  import { bossArt } from '../bossArt';
  import { motionPreference } from '../motion/preference.svelte';

  let {
    still,
    animated = null,
    class: className,
    loading = 'eager',
  }: {
    /** The still entry art URL, or null where there is none. */
    still: string | null;
    /** The looping MP4 URL, or null where the deployment has none. */
    animated?: string | null;
    class: string;
    /** The still's loading hint (a board card defers it). */
    loading?: 'eager' | 'lazy';
  } = $props();

  // The URLs that failed; a different source is tried afresh.
  let failedVideo = $state<string | null>(null);
  let failedStill = $state<string | null>(null);
  const art = $derived(bossArt({ still, animated, reducedMotion: motionPreference.reduced, videoFailed: failedVideo === animated, stillFailed: failedStill === still }));

  const playInView: Attachment<HTMLVideoElement> = (video) => {
    const observer = new IntersectionObserver(([entry]) => {
      // A pause can interrupt a pending play(); that rejection is expected.
      if (entry?.isIntersecting) video.play().catch(() => {});
      else video.pause();
    });
    observer.observe(video);
    // Once removed it is no longer observed, so pause it here or it keeps decoding.
    return () => {
      video.pause();
      observer.disconnect();
    };
  };
</script>

<!-- Keyed by source: a new boss (or the switch to the still) builds a fresh element, never showing the previous frame. -->
{#key art?.src}
  {#if art?.kind === 'video'}
    {@const src = art.src}
    <video class={className} {src} poster={art.poster} muted autoplay loop playsinline preload="metadata" disablepictureinpicture disableremoteplayback aria-hidden="true" onerror={() => (failedVideo = src)} {@attach playInView}></video>
  {:else if art}
    {@const src = art.src}
    <img class={className} {src} alt="" {loading} decoding="async" onerror={() => (failedStill = src)} />
  {/if}
{/key}
