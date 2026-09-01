/**
 * KT-540 — modality detection.
 *
 * The carousel walks images AND videos in one sequence, so the old
 * "is it an image" boolean no longer answers the question.
 */
import { describe, it, expect } from 'vitest';
import { mediaKind, isViewableMedia, isImageFile, isVideoFile } from '../mediaKind';
import type { ContextFile } from '../../types/generated';

function file(p: Partial<ContextFile>): ContextFile {
  return {
    id: 'f', discussion_id: 'd', filename: 'x.bin', mime_type: '',
    original_size: 0, extracted_size: 0, disk_path: '/tmp/x', message_id: null,
    created_at: '2026-01-01T00:00:00Z',
    ...p,
  } as ContextFile;
}

describe('mediaKind', () => {
  it('classifies by mime type first', () => {
    expect(mediaKind(file({ mime_type: 'video/mp4', filename: 'a.bin' }))).toBe('video');
    expect(mediaKind(file({ mime_type: 'image/png', filename: 'a.bin' }))).toBe('image');
  });

  it('falls back to the extension for legacy rows stored as text/plain', () => {
    expect(mediaKind(file({ mime_type: 'text/plain', filename: 'clip.mp4' }))).toBe('video');
    expect(mediaKind(file({ mime_type: 'text/plain', filename: 'shot.png' }))).toBe('image');
  });

  it('treats a file with no bytes on disk as not viewable', () => {
    // Without a disk_path the browser has nothing to render, whatever the
    // filename suggests.
    expect(mediaKind(file({ mime_type: 'video/mp4', disk_path: null }))).toBe('other');
    expect(isViewableMedia(file({ mime_type: 'video/mp4', disk_path: null }))).toBe(false);
  });

  it('excludes documents from the carousel', () => {
    for (const name of ['report.pdf', 'notes.md', 'data.csv', 'archive.zip']) {
      expect(mediaKind(file({ filename: name, mime_type: 'application/octet-stream' }))).toBe('other');
      expect(isViewableMedia(file({ filename: name, mime_type: 'application/octet-stream' }))).toBe(false);
    }
  });

  it('keeps the image and video predicates mutually exclusive', () => {
    const video = file({ mime_type: 'video/mp4' });
    expect(isVideoFile(video)).toBe(true);
    expect(isImageFile(video)).toBe(false);

    const image = file({ mime_type: 'image/webp' });
    expect(isImageFile(image)).toBe(true);
    expect(isVideoFile(image)).toBe(false);
  });

  it('recognises the formats these providers actually return', () => {
    // OpenRouter returns mp4 for video and png for image.
    expect(mediaKind(file({ filename: 'video-abc.mp4', mime_type: 'video/mp4' }))).toBe('video');
    expect(mediaKind(file({ filename: 'image-abc.png', mime_type: 'image/png' }))).toBe('image');
  });
});
