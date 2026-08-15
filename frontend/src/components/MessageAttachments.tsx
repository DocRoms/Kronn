// 0.8.8 — render the files a user pinned to a message (Option B per-message
// attachments). Images become thumbnails fetched as auth'd blobs (an `<img
// src>` can't carry the auth header, so we fetch → object URL → revoke on
// unmount). Non-image files (no disk_path on the backend) render as a filename
// chip. Lives in its own file so the blob lifecycle is unit-testable in
// isolation from the heavy MessageBubble.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight, Download, ExternalLink, FileText, Image as ImageIcon, Loader2, MessageSquare, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions as discussionsApi } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';

type T = (key: string, ...args: (string | number)[]) => string;

const IMAGE_FILENAME = /\.(?:png|jpe?g|gif|webp|svg|bmp|tiff?|ico)$/i;

function formatKb(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function isImageFile(file: ContextFile): boolean {
  // Raw text and arbitrary files are disk-backed too, so disk_path alone does
  // not mean the browser can render a thumbnail. Keep the extension fallback
  // for legacy image rows that were stored with mime_type=text/plain.
  return !!file.disk_path
    && (file.mime_type.startsWith('image/') || IMAGE_FILENAME.test(file.filename));
}

function AttachmentThumb({ file, url, failed, t, onOpen, variant, onNavigateMessage }: {
  file: ContextFile;
  url?: string;
  failed: boolean;
  t: T;
  onOpen: () => void;
  variant: 'message' | 'library';
  onNavigateMessage?: (messageId: string) => void;
}) {
  const isImage = isImageFile(file);
  const meta = `${file.filename} (${formatKb(file.original_size)})`;
  const messageId = file.message_id;
  const [downloading, setDownloading] = useState(false);
  const [downloadFailed, setDownloadFailed] = useState(false);

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
  ) : (
    // Non-image, or an image whose bytes failed to load → filename chip.
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
}: {
  files: ContextFile[];
  discussionId: string;
  t: T;
  variant?: 'message' | 'library';
  onNavigateMessage?: (messageId: string) => void;
}) {
  const imageFiles = useMemo(() => files.filter(isImageFile), [files]);
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [failedIds, setFailedIds] = useState<Set<string>>(() => new Set());
  const [selectedId, setSelectedId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    const objectUrls: string[] = [];
    for (const file of imageFiles) {
      discussionsApi.contextFileBlob(discussionId, file.id)
        .then((blob: Blob) => {
          if (cancelled) return;
          const objectUrl = URL.createObjectURL(blob);
          objectUrls.push(objectUrl);
          setUrls(prev => ({ ...prev, [file.id]: objectUrl }));
          setFailedIds(prev => {
            if (!prev.has(file.id)) return prev;
            const next = new Set(prev);
            next.delete(file.id);
            return next;
          });
        })
        .catch(() => {
          if (!cancelled) setFailedIds(prev => new Set(prev).add(file.id));
        });
    }
    return () => {
      cancelled = true;
      for (const objectUrl of objectUrls) URL.revokeObjectURL(objectUrl);
    };
  }, [discussionId, imageFiles]);

  const viewableImages = imageFiles.filter(file => !!urls[file.id]);
  const selectedIndex = selectedId
    ? viewableImages.findIndex(file => file.id === selectedId)
    : -1;
  const selectedFile = selectedIndex >= 0 ? viewableImages[selectedIndex] : null;

  const moveSelection = useCallback((delta: number) => {
    if (viewableImages.length < 2) return;
    const current = selectedId
      ? viewableImages.findIndex(file => file.id === selectedId)
      : 0;
    const next = (Math.max(0, current) + delta + viewableImages.length) % viewableImages.length;
    setSelectedId(viewableImages[next].id);
  }, [selectedId, viewableImages]);

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
                {selectedIndex + 1} / {viewableImages.length}
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
              {viewableImages.length > 1 && (
                <button
                  type="button"
                  className="disc-image-lightbox-nav previous"
                  onClick={() => moveSelection(-1)}
                  aria-label={t('disc.attachmentPrevious')}
                >
                  <ChevronLeft size={28} />
                </button>
              )}
              <img src={urls[selectedFile.id]} alt={selectedFile.filename} />
              {viewableImages.length > 1 && (
                <button
                  type="button"
                  className="disc-image-lightbox-nav next"
                  onClick={() => moveSelection(1)}
                  aria-label={t('disc.attachmentNext')}
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
