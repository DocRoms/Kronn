// 0.8.8 — render the files a user pinned to a message (Option B per-message
// attachments). Images become thumbnails fetched as auth'd blobs (an `<img
// src>` can't carry the auth header, so we fetch → object URL → revoke on
// unmount). Non-image files (no disk_path on the backend) render as a filename
// chip. Lives in its own file so the blob lifecycle is unit-testable in
// isolation from the heavy MessageBubble.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChevronLeft, ChevronRight, Download, ExternalLink, FileText, Image as ImageIcon, Loader2, MessageSquare, Scissors, Sparkles, Trash2, X } from 'lucide-react';
import type { ContextFile } from '../types/generated';
import { discussions as discussionsApi, media, type ExternalApiConnectionView, type MediaModality, type MediaReferenceMode } from '../lib/api';
import { triggerDownload } from '../lib/downloadBlob';
import { isImageFile, isVideoFile, isViewableMedia } from '../lib/mediaKind';
import { extractLastFrame, LastFrameError, lastFrameFilename } from '../lib/lastFrame';
import { MediaPlayer } from './MediaPlayer';

type T = (key: string, ...args: (string | number)[]) => string;
const EMPTY_GENERATION_CONNECTIONS: ExternalApiConnectionView[] = [];

export type ImageGenerationRequest = {
  assetId: string;
  modality: MediaModality;
  slotKey: string;
  referenceMode: MediaReferenceMode;
};

function formatKb(bytes: number): string {
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function AiGeneratedBadge({ t }: { t: T }) {
  return (
    <span className="disc-ai-generated-badge" data-testid="ai-generated-badge">
      <Sparkles size={10} aria-hidden="true" />
      {t('disc.assets.aiGenerated')}
    </span>
  );
}

function AiGenerationDetails({ file, t }: { file: ContextFile; t: T }) {
  if (!file.ai_generation) return null;
  return (
    <div
      className="disc-ai-generation-details"
      role="note"
      aria-label={t('disc.assets.aiGenerated')}
      data-testid="ai-generation-details"
    >
      <div className="disc-ai-generation-heading">
        <span className="disc-ai-generation-label">
          <Sparkles size={13} aria-hidden="true" />
          {t('disc.assets.aiGenerated')}
        </span>
        <span className="disc-ai-generation-model">
          <span>{t('disc.assets.aiModel')}</span>
          <code>{file.ai_generation.model}</code>
        </span>
      </div>
      <p className="disc-ai-generation-prompt">
        <strong>{t('disc.assets.aiPrompt')}</strong>
        <span>{file.ai_generation.prompt}</span>
      </p>
    </div>
  );
}

/// Where a picture came from when it was not generated: a frame decoded out of
/// a clip of this discussion. Said plainly, and never as "AI generated" — that
/// badge is an attestation, and nothing was produced here.
function ExtractedFromDetails({ file, source, t, onOpenSource }: {
  file: ContextFile;
  source: ContextFile | null;
  t: T;
  onOpenSource?: (assetId: string) => void;
}) {
  const sourceId = file.extracted_from_asset_id;
  if (!sourceId) return null;
  return (
    <div
      className="disc-ai-generation-details"
      role="note"
      aria-label={t('disc.assets.extractedFrom')}
      data-testid="extracted-from-details"
    >
      <div className="disc-ai-generation-heading">
        <span className="disc-ai-generation-label">
          <Scissors size={13} aria-hidden="true" />
          {t('disc.assets.extractedFrom')}
        </span>
      </div>
      <p className="disc-ai-generation-prompt">
        <span>{t('disc.assets.extractedFromExplained')}</span>
        {onOpenSource && (
          // The clip is one click away: a provenance nobody can follow is a
          // sentence, not a link back to the thing it names.
          <button
            type="button"
            className="disc-media-msg-source-open"
            onClick={() => onOpenSource(sourceId)}
            data-testid="extracted-from-open-source"
          >
            {source ? source.filename : t('disc.assets.openSourceVideo')}
          </button>
        )}
      </p>
    </div>
  );
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

  const previewContent = isImage && !failed ? (
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
  const preview = file.ai_generation && (isImage || isVideo) ? (
    <span className="disc-attach-generated">
      {previewContent}
      <AiGeneratedBadge t={t} />
    </span>
  ) : previewContent;

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
  openRequest,
  onDeleted,
  onExtracted,
  generationConnections = EMPTY_GENERATION_CONNECTIONS,
  onGenerateFromImage,
}: {
  files: ContextFile[];
  discussionId: string;
  t: T;
  variant?: 'message' | 'library';
  /** Enables deletion. Absent, no delete control is shown at all: a surface
   *  that cannot refresh its own list must not offer to shorten it. */
  onDeleted?: (fileId: string) => void;
  /** Enables "keep the last frame" on a clip. Same rule as `onDeleted`: a
   *  surface that cannot show the new file must not offer to create it. */
  onExtracted?: (file: ContextFile) => void;
  onNavigateMessage?: (messageId: string) => void;
  /// Full sequence to browse once one thumbnail is opened. The grid still
  /// shows `files`; this is what the arrows walk through, so opening an image
  /// under one message reaches every image AND clip of the discussion instead
  /// of stopping at that message's own attachments.
  carouselScope?: ContextFile[];
  /** Controlled one-shot open request from outside the attachment grid. */
  openRequest?: { assetId: string; nonce: number } | null;
  /** Configured media slots whose input-image capability is checked before an
   * action is offered in the viewer. */
  generationConnections?: ExternalApiConnectionView[];
  onGenerateFromImage?: (request: ImageGenerationRequest) => void;
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
  // Two steps on purpose: this removes bytes from disk, and the control sits
  // next to "close". `error` is kept apart from the confirmation so a failure
  // stays on screen instead of being wiped by the next render.
  // KT-587 — all three belong to ONE asset, so they carry its id rather than
  // being wiped by an effect when the reader walks to the next picture. A
  // confirmation surviving that walk would delete a file nobody asked about.
  const [assetState, setAssetState] = useState<{
    id: string | null; confirming: boolean; deleteError: string | null; extractError: string | null;
  }>({ id: null, confirming: false, deleteError: null, extractError: null });
  const [deleting, setDeleting] = useState(false);
  const [extracting, setExtracting] = useState(false);
  const patchAssetState = useCallback((patch: {
    confirming?: boolean; deleteError?: string | null; extractError?: string | null;
  }) => {
    setAssetState(current => ({
      id: selectedId,
      confirming: false,
      deleteError: null,
      extractError: null,
      // Anything already recorded for THIS asset survives the patch; a value
      // left over from the previous one does not.
      ...(current.id === selectedId ? current : {}),
      ...patch,
    }));
  }, [selectedId]);
  const objectUrlsRef = useRef<Map<string, string>>(new Map());
  const inFlightRef = useRef<Set<string>>(new Set());
  const generationRef = useRef(0);
  const mountedRef = useRef(false);
  const discussionRef = useRef(discussionId);
  const cleanupSequenceRef = useRef(0);
  // State rather than a ref: the request is answered during render, and a ref
  // written there is exactly what `react-hooks/refs` exists to catch.
  const [handledOpenNonce, setHandledOpenNonce] = useState<number | null>(null);
  const [inputModes, setInputModes] = useState<Record<string, MediaReferenceMode | null>>({});
  const generationActions = useMemo(() => {
    const actions: Array<{ modality: MediaModality; slotKey: string; referenceMode: MediaReferenceMode }> = [];
    for (const connection of generationConnections) {
      for (const modality of ['image', 'video'] as const) {
        if (actions.some(action => action.modality === modality)) continue;
        const slotKey = `${connection.id}:${modality}`;
        const referenceMode = inputModes[slotKey];
        if (referenceMode) actions.push({ modality, slotKey, referenceMode });
      }
    }
    return actions;
  }, [generationConnections, inputModes]);

  // Do not infer a provider's input support. Until this request resolves (or
  // after it fails), the viewer offers no paid-generation action at all.
  useEffect(() => {
    let cancelled = false;
    const slots = generationConnections.flatMap(connection => (
      (['image', 'video'] as const).flatMap(modality => {
        const model = modality === 'image' ? connection.image_model : connection.video_model;
        return model?.trim() ? [{ key: `${connection.id}:${modality}`, connectionId: connection.id, modality }] : [];
      })
    ));
    setInputModes({});
    void Promise.all(slots.map(async slot => {
      try {
        const result = await media.capabilities(slot.connectionId, slot.modality);
        const mode = slot.modality === 'image'
          ? (result.capabilities?.max_input_references ?? 0) > 0 ? 'reference' : null
          : result.capabilities?.frame_positions?.[0] ?? null;
        return [slot.key, mode] as const;
      } catch {
        return [slot.key, null] as const;
      }
    })).then(entries => {
      if (!cancelled) setInputModes(Object.fromEntries(entries));
    });
    return () => { cancelled = true; };
  }, [generationConnections]);

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
      setHandledOpenNonce(null);
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



  // KT-556 — keep the clip's last picture as a file of this discussion, so a
  // follow-up generation can start from it without a download and a re-upload.
  // The frame is decoded by the player's own element: the backend cannot read
  // these clips at all.
  const keepLastFrame = useCallback(async (file: ContextFile, src: string) => {
    setExtracting(true);
    patchAssetState({ extractError: null });
    try {
      const frame = await extractLastFrame(src);
      const image = new File([frame.blob], lastFrameFilename(file.filename), { type: 'image/png' });
      // Named as coming from this clip: the server gives it its own message,
      // so it is never a pending attachment waiting on the next send — and
      // deleting an unrelated message can no longer take it away.
      const uploaded = await discussionsApi.uploadContextFile(file.discussion_id, image, file.id);
      onExtracted?.(uploaded.file);
      // Landing on the fresh image is the answer to "did it work": the reader
      // sees the frame instead of a message claiming one exists.
      setSelectedId(uploaded.file.id);
    } catch (e) {
      patchAssetState({
        extractError: e instanceof LastFrameError
          ? t(`disc.media.lastFrame.error.${e.cause_}`)
          : e instanceof Error ? e.message : String(e),
      });
    } finally {
      setExtracting(false);
    }
  }, [onExtracted, patchAssetState, t]);

  const forSelected = assetState.id === selectedId ? assetState : null;
  const confirmingDelete = forSelected?.confirming ?? false;
  const deleteError = forSelected?.deleteError ?? null;
  const extractError = forSelected?.extractError ?? null;
  const selectedIndex = selectedId
    ? carouselFiles.findIndex(file => file.id === selectedId)
    : -1;
  const selectedFile = selectedIndex >= 0 ? carouselFiles[selectedIndex] : null;

  // KT-587 — a request to open one asset is an event, not state to keep in
  // step. Adjusting during render puts the carousel on it in the same pass;
  // the effect showed the previous asset for one frame first.
  if (
    openRequest
    && handledOpenNonce !== openRequest.nonce
    && carouselFiles.some(file => file.id === openRequest.assetId)
  ) {
    setHandledOpenNonce(openRequest.nonce);
    setSelectedId(openRequest.assetId);
  }

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
          data-asset-id={selectedFile.id}
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
              {onExtracted && isVideoFile(selectedFile) && urls[selectedFile.id] && (
                // Offered only once the bytes are here: without them there is
                // nothing to decode, and the button would promise a frame it
                // cannot produce.
                <button
                  type="button"
                  className="disc-image-lightbox-action"
                  disabled={extracting}
                  onClick={() => { void keepLastFrame(selectedFile, urls[selectedFile.id]); }}
                  aria-label={t('disc.media.lastFrame.action')}
                  title={t('disc.media.lastFrame.action')}
                  data-testid="attachment-last-frame"
                >
                  {extracting ? <Loader2 size={18} /> : <Scissors size={18} />}
                </button>
              )}
              {isImageFile(selectedFile) && onGenerateFromImage && (
                <>
                  {generationActions.map(({ modality, slotKey, referenceMode }) => (
                    <button
                      key={slotKey}
                      type="button"
                      className="disc-image-lightbox-action"
                      onClick={() => onGenerateFromImage({
                        assetId: selectedFile.id,
                        modality,
                        slotKey,
                        referenceMode,
                      })}
                      aria-label={t(`disc.media.generateFromImage.${modality}`)}
                      title={t(`disc.media.generateFromImage.${modality}`)}
                      data-testid={`attachment-generate-${modality}`}
                    >
                      {modality === 'image' ? <ImageIcon size={18} /> : <Sparkles size={18} />}
                    </button>
                  ))}
                </>
              )}
              {onDeleted && (confirmingDelete ? (
                <button
                  type="button"
                  className="disc-image-lightbox-action disc-image-lightbox-danger"
                  disabled={deleting}
                  onClick={() => {
                    const fileId = selectedFile.id;
                    setDeleting(true);
                    patchAssetState({ deleteError: null });
                    discussionsApi.deleteContextFile(discussionId, fileId)
                      .then(() => {
                        // Closed rather than advanced: silently landing on the
                        // neighbouring media would look like the wrong file
                        // was deleted.
                        setSelectedId(null);
                        patchAssetState({ confirming: false });
                        onDeleted(fileId);
                      })
                      // The asset stays on screen: it must never disappear
                      // from the UI without having been deleted on the server.
                      .catch((e: unknown) => patchAssetState({ deleteError: e instanceof Error ? e.message : String(e) }))
                      .finally(() => setDeleting(false));
                  }}
                  aria-label={t('disc.attachmentDeleteConfirm')}
                  title={t('disc.attachmentDeleteConfirm')}
                  data-testid="attachment-delete-confirm"
                >
                  {t('disc.attachmentDeleteConfirm')}
                </button>
              ) : (
                <button
                  type="button"
                  className="disc-image-lightbox-action"
                  onClick={() => patchAssetState({ confirming: true, deleteError: null })}
                  aria-label={t('disc.attachmentDelete')}
                  title={t('disc.attachmentDelete')}
                  data-testid="attachment-delete"
                >
                  <Trash2 size={18} />
                </button>
              ))}
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
            {deleteError && (
              <p className="disc-image-lightbox-error" role="alert" data-testid="attachment-delete-error">
                {deleteError}
              </p>
            )}
            {extractError && (
              <p className="disc-image-lightbox-error" role="alert" data-testid="attachment-last-frame-error">
                {extractError}
              </p>
            )}
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
              <div className="disc-image-lightbox-asset">
                <div className="disc-image-lightbox-media-frame">
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
                </div>
                <AiGenerationDetails file={selectedFile} t={t} />
                <ExtractedFromDetails
                  file={selectedFile}
                  source={carouselFiles.find(item => item.id === selectedFile.extracted_from_asset_id) ?? null}
                  t={t}
                  onOpenSource={assetId => {
                    // Walking to the clip inside the same viewer: it is part
                    // of this sequence, so nothing has to be reopened.
                    if (carouselFiles.some(item => item.id === assetId)) setSelectedId(assetId);
                  }}
                />
              </div>
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
