// Modality detection for an attachment.
//
// Replaces the previous image-only predicate: the carousel has to walk images
// AND videos in one sequence, so a boolean "is it an image" no longer answers
// the question being asked.

import type { ContextFile } from '../types/generated';

export type MediaKind = 'image' | 'video' | 'other';

const IMAGE_EXTENSIONS = /\.(?:png|jpe?g|gif|webp|svg|bmp|tiff?|ico|avif)$/i;
const VIDEO_EXTENSIONS = /\.(?:mp4|webm|mov|m4v|ogv)$/i;

/**
 * A file the browser can display inline needs its bytes on disk. Extensions
 * remain a fallback: legacy rows were stored with `mime_type=text/plain`.
 */
export function mediaKind(file: ContextFile): MediaKind {
  if (!file.disk_path) return 'other';
  if (file.mime_type.startsWith('image/') || IMAGE_EXTENSIONS.test(file.filename)) return 'image';
  if (file.mime_type.startsWith('video/') || VIDEO_EXTENSIONS.test(file.filename)) return 'video';
  return 'other';
}

/** True when the file belongs in the carousel at all. */
export function isViewableMedia(file: ContextFile): boolean {
  return mediaKind(file) !== 'other';
}

/** Kept for the thumbnail path, which still treats images specially. */
export function isImageFile(file: ContextFile): boolean {
  return mediaKind(file) === 'image';
}

export function isVideoFile(file: ContextFile): boolean {
  return mediaKind(file) === 'video';
}
