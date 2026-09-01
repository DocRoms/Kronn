/**
 * KT-540 — media player contract.
 *
 * The properties that matter are the ones a user would notice going wrong:
 * autoplay only inside the deliberately opened viewer, megabytes pulled for
 * a clip nobody opened, and a clip stretched because the provider ignored
 * the requested geometry.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MediaPlayer } from '../MediaPlayer';

afterEach(cleanup);

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}|${args.join('|')}` : key;

describe('MediaPlayer', () => {
  it('autoplays muted once the viewer deliberately mounts the clip', () => {
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    const video = screen.getByTestId('media-player-video') as HTMLVideoElement;
    expect(video.hasAttribute('autoplay')).toBe(true);
    expect(video.autoplay).toBe(true);
    expect(video.muted).toBe(true);
  });

  it('preloads metadata only, not the whole clip', () => {
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    // A generated clip weighs megabytes; a card merely on screen must not
    // pull them.
    expect(screen.getByTestId('media-player-video').getAttribute('preload')).toBe('metadata');
  });

  it('exposes native controls and an accessible name', () => {
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    const video = screen.getByTestId('media-player-video');
    expect(video.hasAttribute('controls')).toBe(true);
    expect(video.getAttribute('aria-label')).toContain('clip.mp4');
  });

  it('applies the real geometry when it is known', () => {
    // 864x496 is what a "480p 16:9" request actually produced.
    render(<MediaPlayer src="blob:x" filename="clip.mp4" width={864} height={496} t={t} />);
    const video = screen.getByTestId('media-player-video') as HTMLVideoElement;
    expect(video.style.aspectRatio.replace(/\s/g, '')).toBe('864/496');
  });

  it('applies no ratio at all when the geometry is unknown', () => {
    // Forcing a guessed ratio is how black bars and stretch appear.
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    expect((screen.getByTestId('media-player-video') as HTMLVideoElement).style.aspectRatio).toBe('');

    cleanup();
    // A half-known geometry is still unknown.
    render(<MediaPlayer src="blob:x" filename="c.mp4" width={864} t={t} />);
    expect((screen.getByTestId('media-player-video') as HTMLVideoElement).style.aspectRatio).toBe('');
  });

  it('offers Picture-in-Picture only when the document allows it', () => {
    const original = (document as Document & { pictureInPictureEnabled?: boolean }).pictureInPictureEnabled;

    Object.defineProperty(document, 'pictureInPictureEnabled', { value: true, configurable: true });
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    expect(screen.getByTestId('media-player-pip')).toBeTruthy();

    cleanup();
    Object.defineProperty(document, 'pictureInPictureEnabled', { value: false, configurable: true });
    render(<MediaPlayer src="blob:x" filename="clip.mp4" t={t} />);
    // No dead control when the browser would refuse it anyway.
    expect(screen.queryByTestId('media-player-pip')).toBeNull();

    Object.defineProperty(document, 'pictureInPictureEnabled', { value: original, configurable: true });
  });
});
