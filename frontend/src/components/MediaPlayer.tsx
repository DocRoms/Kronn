// Video player for a generated or attached media file.
//
// Isolated in its own component on purpose: the player implementation is a
// decision we may revisit (video.js and friends), and keeping it behind one
// component means that change stays local instead of spreading through the
// carousel and the message bubble.
//
// No autoplay, ever. A discussion that starts making noise because a job
// finished is worse than one extra click.
import { useCallback, useRef, useState } from 'react';
import './MediaPlayer.css';

type T = (key: string, ...args: (string | number)[]) => string;

export function MediaPlayer({
  src,
  filename,
  /** Real dimensions read from the produced file — the provider does not
   * honour the requested geometry, so an aspect ratio derived from the request
   * produces black bars or stretch. */
  width,
  height,
  t,
}: {
  src: string;
  filename: string;
  width?: number | null;
  height?: number | null;
  t: T;
}) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  // Picture-in-Picture comes from the browser, not from a player library.
  // Read once at mount rather than pushed through an effect, which would
  // cascade a render for a value that cannot change. Defensive: some contexts
  // expose the property but disable it.
  const [pipAvailable] = useState(
    () =>
      typeof document !== 'undefined' &&
      'pictureInPictureEnabled' in document &&
      (document as Document & { pictureInPictureEnabled?: boolean }).pictureInPictureEnabled === true,
  );

  const togglePip = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    try {
      if (document.pictureInPictureElement) {
        await document.exitPictureInPicture();
      } else {
        await video.requestPictureInPicture();
      }
    } catch {
      // A refusal (user gesture policy, unsupported track) must not break the
      // player; native controls still offer their own entry point.
    }
  }, []);

  // Only set when both are known, so an unknown geometry falls back to the
  // container instead of forcing a wrong ratio.
  const aspectRatio = width && height ? `${width} / ${height}` : undefined;

  return (
    <div className="media-player" data-testid="media-player">
      <video
        ref={videoRef}
        className="media-player-video"
        data-testid="media-player-video"
        src={src}
        controls
        // Metadata only: a generated clip weighs megabytes, and a card that is
        // merely on screen must not pull them.
        preload="metadata"
        playsInline
        style={aspectRatio ? { aspectRatio } : undefined}
        aria-label={t('disc.media.playerLabel', filename)}
      />
      {pipAvailable && (
        <button
          type="button"
          className="media-player-pip"
          data-testid="media-player-pip"
          onClick={() => void togglePip()}
          title={t('disc.media.pictureInPicture')}
          aria-label={t('disc.media.pictureInPicture')}
        >
          ⧉
        </button>
      )}
    </div>
  );
}
