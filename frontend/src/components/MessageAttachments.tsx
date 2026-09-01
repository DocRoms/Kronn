// 0.8.8 — render the files a user pinned to a message (Option B per-message
// attachments). Images become thumbnails fetched as auth'd blobs (an `<img
// src>` can't carry the auth header, so we fetch → object URL → revoke on
// unmount). Non-image files (no disk_path on the backend) render as a filename
// chip. Lives in its own file so the blob lifecycle is unit-testable in
// isolation from the heavy MessageBubble.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight, Download, ExternalLink, FileText, Image as ImageIcon, Loader2, MessageSquare, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions as discussionsApi } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';
import { isImageFile, isVideoFile, isViewableMedia } from '../lib/mediaKind';
import { MediaPlayer } from './MediaPlayer';

type T = (key: string, ...args: (string | number)[]) => string;

function formatKb(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function AttachmentThumb({ file, url, failed, t, onOpen, onPrepareVideo, variant, onNavigateMessage }: {
  file: ContextFile;
  url?: string;
  failed: boolean;
  t: T;
  onOpen: () => void;
  onPrepareVideo?: () => void;
  variant: 'message' | 'library';
  onNavigateMessage?: (messageId: string) => void;
}) {
  const isImage = isImageFile(file);
  const isVideo = isVideoFile(file);
  const meta = `${file.filename} (${formatKb(file.original_size)})`;
  const messageId = file.message_id;
  const videoThumbRef = useRef<HTMLButtonElement | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [downloadFailed, setDownloadFailed] = useState(false);

  // A real video thumbnail needs the authenticated blob. Load it only when a
  // library card enters the viewport; message rows keep the cheap badge and
  // fetch only after the user opens the clip.
  useEffect(() => {
    if (!isVideo || variant !== 'library' || url || failed || !onPrepareVideo) return;
    const node = videoThumbRef.current;
    if (!node || typeof IntersectionObserver === 'undefined') {
      onPrepareVideo();
      return;
    }
    const observer = new IntersectionObserver(entries => {
      if (!entries.some(entry => entry.isIntersecting)) return;
      observer.disconnect();
      onPrepareVideo();
    }, { rootMargin: '160px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, [failed, isVideo, onPrepareVideo, url, variant]);

  const downloadFile = async () => {
    if (downloading || !file.disk_path) return;
    setDownloading(true);
    setDownloadFailed(false);
    try {
      const blob = await discussionsApi.contextFileBlob(file.discussion_id, file.id);
      triggerDownload(file.filename, blob);
    } catch {
      setDownloadFailed(true);
    } finally {
      setDownloading(false);
    }
  };

  const preview = isImage && !failed ? (
      <button
        type="button"
        className="disc-attach-thumb"
        onClick={onOpen}
        disabled={!url}
        title={meta}
        aria-label={t('disc.attachmentImage', file.filename)}
      >
        {url
          ? <img src={url} alt={file.filename} loading="lazy" />
          : <span className="disc-attach-thumb-loading" aria-hidden="true"><ImageIcon size={14} /></span>}
      </button>
  ) : isVideo && !failed ? (
    // A video gets a real, openable thumbnail — no <video> here, so a row that
    // is merely on screen pulls nothing. The bytes are fetched when the
    // carousel opens on it.
    <button
      ref={videoThumbRef}
      type="button"
      className="disc-attach-thumb disc-attach-thumb--video"
      onClick={onOpen}
      title={meta}
      aria-label={t('disc.media.playerLabel', file.filename)}
      data-testid="attach-video-thumb"
    >
      {url && (
        <video
          className="disc-attach-video-poster"
          src={url}
          preload="metadata"
          muted
          playsInline
          aria-hidden="true"
          data-testid="attach-video-poster"
          onLoadedMetadata={event => {
            const video = event.currentTarget;
            if (Number.isFinite(video.duration) && video.duration > 0) {
              video.currentTime = Math.min(0.1, video.duration / 10);
            }
          }}
        />
      )}
      <span className="disc-attach-video-badge" aria-hidden="true">▶</span>
      <span className="disc-attach-video-kind">{t('disc.media.videoBadge')}</span>
    </button>
  ) : (
    // Anything else, or a media whose bytes failed to load → filename chip.
    <span className="disc-attach-chip" title={meta} data-testid="attach-chip">
      <FileText size={11} />
      <span className="disc-attach-chip-name">{file.filename}</span>
    </span>
  );

  if (variant === 'message') return preview;

  return (
    <article className="disc-asset-card" data-testid="discussion-asset-card">
      <div className="disc-asset-card-preview">{preview}</div>
      <div className="disc-asset-card-body">
        <span className="disc-asset-card-name" title={file.filename}>{file.filename}</span>
        <span className="disc-asset-card-meta">
          {formatKb(file.original_size)} · {new Date(file.created_at).toLocaleDateString()}
        </span>
      </div>
      <div className="disc-asset-card-actions">
        {file.disk_path && (
          <button
            type="button"
            className="disc-asset-card-action"
            data-error={downloadFailed}
            onClick={() => void downloadFile()}
            disabled={downloading}
            title={downloadFailed ? t('disc.assets.downloadFailed') : t('disc.assets.download')}
            aria-label={downloadFailed
              ? t('disc.assets.downloadFailedFor', file.filename)
              : t('disc.assets.downloadFor', file.filename)}
          >
            {downloading ? <Loader2 size={13} className="spin" /> : <Download size={13} />}
          </button>
        )}
        {messageId && (
          <button
            type="button"
            className="disc-asset-card-action"
            onClick={() => onNavigateMessage?.(messageId)}
            title={t('disc.assets.goToMessage')}
            aria-label={t('disc.assets.goToMessageFor', file.filename)}
          >
            <MessageSquare size={13} />
          </button>
        )}
      </div>
      {!messageId && (
        <span className="disc-asset-pending">{t('disc.assets.pending')}</span>
      )}
    </article>
  );
}

export function MessageAttachments({
  files,
  discussionId,
  t,
  variant = 'message',
  onNavigateMessage,
  carouselScope,
}: {
  files: ContextFile[];
  discussionId: string;
  t: T;
  variant?: 'message' | 'library';
  onNavigateMessage?: (messageId: string) => void;
  /// Full sequence to browse once one thumbnail is opened. The grid still
  /// shows `files`; this is what the arrows walk through, so opening an image
  /// under one message reaches every image AND clip of the discussion instead
  /// of stopping at that message's own attachments.
  carouselScope?: ContextFile[];
}) {
  const imageFiles = useMemo(() => files.filter(isImageFile), [files]);
  // Membership is decided on METADATA, not on a loaded blob: filtering on
  // `urls` excluded a video nobody had downloaded yet, and an image still in
  // flight, so the carousel silently skipped entries.
  const carouselFiles = useMemo(() => {
    const sequence = (carouselScope ?? files).filter(isViewableMedia);
    // A thumbnail must always be reachable from the sequence it opens: a scope
    // that filtered or paginated the grid away would otherwise open on nothing.
    const known = new Set(sequence.map(file => file.id));
    const orphans = files.filter(file => isViewableMedia(file) && !known.has(file.id));
    return [...sequence, ...orphans];
  }, [carouselScope, files]);
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [failedIds, setFailedIds] = useState<Set<string>>(() => new Set());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const objectUrlsRef = useRef<Map<string, string>>(new Map());
  const inFlightRef = useRef<Set<string>>(new Set());
  const generationRef = useRef(0);
  const mountedRef = useRef(false);
  const discussionRef = useRef(discussionId);
  const cleanupSequenceRef = useRef(0);

  const releaseMediaUrls = useCallback(() => {
    generationRef.current += 1;
    for (const objectUrl of objectUrlsRef.current.values()) {
      URL.revokeObjectURL(objectUrl);
    }
    objectUrlsRef.current.clear();
    inFlightRef.current.clear();
  }, []);

  const finalizeUnmount = useCallback((cleanupSequence: number) => {
    if (mountedRef.current || cleanupSequenceRef.current !== cleanupSequence) return;
    releaseMediaUrls();
  }, [releaseMediaUrls]);

  // Object URLs belong to the discussion, not to a transient selection. The
  // former video effect revoked its URL as soon as setUrls triggered a rerun,
  // then kept that revoked string in state: reopening the clip produced an
  // endless native loader. Cleanup is deferred by one microtask so React
  // StrictMode's synthetic unmount/remount does not download a large clip
  // twice. A real discussion change still clears its URLs synchronously.
  useLayoutEffect(() => {
    if (discussionRef.current !== discussionId) {
      releaseMediaUrls();
      discussionRef.current = discussionId;
      setUrls({});
      setFailedIds(new Set());
      setSelectedId(null);
    }
    mountedRef.current = true;
    cleanupSequenceRef.current += 1;
    return () => {
      mountedRef.current = false;
      const cleanupSequence = ++cleanupSequenceRef.current;
      queueMicrotask(() => finalizeUnmount(cleanupSequence));
    };
  }, [discussionId, finalizeUnmount, releaseMediaUrls]);

  const loadMediaUrl = useCallback((file: ContextFile) => {
    if (objectUrlsRef.current.has(file.id) || inFlightRef.current.has(file.id)) return;
    const generation = generationRef.current;
    inFlightRef.current.add(file.id);
    discussionsApi.contextFileBlob(discussionId, file.id)
      .then((blob: Blob) => {
        if (!mountedRef.current || generation !== generationRef.current) return;
        const objectUrl = URL.createObjectURL(blob);
        objectUrlsRef.current.set(file.id, objectUrl);
        setUrls(prev => ({ ...prev, [file.id]: objectUrl }));
        setFailedIds(prev => {
          if (!prev.has(file.id)) return prev;
          const next = new Set(prev);
          next.delete(file.id);
          return next;
        });
      })
      .catch(() => {
        if (mountedRef.current && generation === generationRef.current) {
          setFailedIds(prev => new Set(prev).add(file.id));
        }
      })
      .finally(() => {
        if (generation === generationRef.current) inFlightRef.current.delete(file.id);
      });
  }, [discussionId]);

  useEffect(() => {
    for (const file of imageFiles) {
      loadMediaUrl(file);
    }
  }, [imageFiles, loadMediaUrl]);

  const selectedIndex = selectedId
    ? carouselFiles.findIndex(file => file.id === selectedId)
    : -1;
  const selectedFile = selectedIndex >= 0 ? carouselFiles[selectedIndex] : null;

  // A clip weighs megabytes, so its bytes are fetched only once it is the one
  // being looked at — never for the whole carousel.
  useEffect(() => {
    if (!selectedFile || !isVideoFile(selectedFile)) return;
    loadMediaUrl(selectedFile);
  }, [loadMediaUrl, selectedFile]);

  const moveSelection = useCallback((delta: number) => {
    if (carouselFiles.length < 2) return;
    const current = selectedId
      ? carouselFiles.findIndex(file => file.id === selectedId)
      : 0;
    const next = (Math.max(0, current) + delta + carouselFiles.length) % carouselFiles.length;
    setSelectedId(carouselFiles[next].id);
  }, [selectedId, carouselFiles]);

  useEffect(() => {
    if (!selectedFile) return;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setSelectedId(null);
      if (event.key === 'ArrowLeft') moveSelection(-1);
      if (event.key === 'ArrowRight') moveSelection(1);
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.body.style.overflow = previousOverflow;
    };
  }, [moveSelection, selectedFile]);

  if (!files.length) return null;
  return (
    <>
      <div
        className={variant === 'library' ? 'disc-asset-library-grid' : 'disc-msg-attachments'}
        data-testid={variant === 'library' ? 'asset-library-grid' : 'msg-attachments'}
      >
        {files.map(file => (
          <AttachmentThumb
            key={file.id}
            file={file}
            url={urls[file.id]}
            failed={failedIds.has(file.id)}
            t={t}
            onOpen={() => setSelectedId(file.id)}
            onPrepareVideo={() => loadMediaUrl(file)}
            variant={variant}
            onNavigateMessage={onNavigateMessage}
          />
        ))}
      </div>
      {selectedFile && createPortal(
        <div
          className="disc-image-lightbox"
          role="dialog"
          aria-modal="true"
          aria-label={t('disc.attachmentGallery')}
          onClick={() => setSelectedId(null)}
        >
          <div className="disc-image-lightbox-stage" onClick={event => event.stopPropagation()}>
            <div className="disc-image-lightbox-toolbar">
              <span className="disc-image-lightbox-name" title={selectedFile.filename}>
                {selectedFile.filename}
              </span>
              <span className="disc-image-lightbox-count">
                {selectedIndex + 1} / {carouselFiles.length}
              </span>
              <a
                className="disc-image-lightbox-action"
                href={urls[selectedFile.id]}
                target="_blank"
                rel="noreferrer"
                aria-label={t('disc.attachmentOpenNewTab')}
                title={t('disc.attachmentOpenNewTab')}
              >
                <ExternalLink size={17} />
              </a>
              <button
                type="button"
                className="disc-image-lightbox-action"
                onClick={() => setSelectedId(null)}
                aria-label={t('disc.attachmentClose')}
                title={t('disc.attachmentClose')}
              >
                <X size={20} />
              </button>
            </div>
            <div className="disc-image-lightbox-content">
              {carouselFiles.length > 1 && (
                <button
                  type="button"
                  className="disc-image-lightbox-nav previous"
                  onClick={() => moveSelection(-1)}
                  aria-label={t('disc.media.carouselPrevious')}
                >
                  <ChevronLeft size={28} />
                </button>
              )}
              {isVideoFile(selectedFile)
                ? urls[selectedFile.id]
                  ? <MediaPlayer
                      src={urls[selectedFile.id]}
                      filename={selectedFile.filename}
                      t={t}
                    />
                  : failedIds.has(selectedFile.id)
                    ? <span className="disc-image-lightbox-empty">{t('disc.attachmentImage', selectedFile.filename)}</span>
                    // Fetched on selection, so a brief placeholder is expected
                    // rather than an empty frame.
                    : <span className="disc-image-lightbox-empty" data-testid="lightbox-loading">
                        <Loader2 size={20} />
                      </span>
                : <img src={urls[selectedFile.id]} alt={selectedFile.filename} />}
              {carouselFiles.length > 1 && (
                <button
                  type="button"
                  className="disc-image-lightbox-nav next"
                  onClick={() => moveSelection(1)}
                  aria-label={t('disc.media.carouselNext')}
                >
                  <ChevronRight size={28} />
                </button>
              )}
            </div>
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
