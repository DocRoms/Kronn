// KT-549 — one durable media bubble at the launch's transcript position.
//
// This component owns the whole message row. Rendering a normal user bubble
// next to a run card made one generation look like two unrelated messages and
// made the card disappear on success. The root stays stable for every state;
// the completed asset opens through the canonical Assets viewer.
import { useEffect, useState } from 'react';
import { ExternalLink, Image as ImageIcon, Sparkles } from 'lucide-react';
import { RunStatusCard } from './RunStatusCard';
import { useT } from '../lib/I18nContext';
import { mediaRunDetails } from '../lib/mediaRunResult';
import { sharedRunStatusCardModel } from '../lib/runStatusCardModel';
import { discussions as discussionsApi } from '../lib/api';
import type { SharedRun } from '../types/generated';

export function InlineMediaJob({
  discussionId,
  messageId,
  prompt,
  run,
  onOpenAsset,
}: {
  discussionId: string;
  messageId: string;
  prompt: string;
  run: SharedRun;
  onOpenAsset: (assetId: string) => void;
}) {
  const { t } = useT();
  const details = mediaRunDetails(run.result);
  const assetId = details?.assetId;
  const modality = details?.modality;
  const source = details?.referenceAssetId && details.referenceMode
    ? { assetId: details.referenceAssetId, mode: details.referenceMode }
    : null;
  const model = sharedRunStatusCardModel(run, 'live');
  // The produced file, shown where it was asked for. `<img src>` cannot carry
  // the auth header, so the bytes come through the API and live as an object
  // URL for as long as this bubble is mounted.
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!assetId) return;
    let objectUrl: string | null = null;
    let cancelled = false;
    discussionsApi.contextFileBlob(discussionId, assetId)
      .then((blob: Blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setPreviewUrl(objectUrl);
      })
      // The bubble keeps its "open the media" button, which reaches the same
      // asset through the viewer: a preview that failed to load is a missing
      // convenience, never a lost result.
      .catch(() => { if (!cancelled) setPreviewUrl(null); });
    return () => {
      cancelled = true;
      setPreviewUrl(null);
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [assetId, discussionId]);

  return (
    <div
      className="disc-msg-row"
      data-role="media"
      data-message-id={messageId}
      data-media-run-id={run.id}
      data-media-asset-id={assetId}
    >
      <article className="disc-msg-bubble disc-media-msg-bubble" data-role="media">
        <header className="disc-media-msg-header">
          <Sparkles size={15} aria-hidden="true" />
          <span>{prompt}</span>
        </header>
        {/* The bubble's own button opens the asset; the card's generic link
            would point at the run page, which answers nothing here. */}
        <RunStatusCard model={model} runId={run.id} hideRunLink />
        {source && (
          // What the clip was built on, said where the clip is. The id is
          // resolved through the discussion's own files: no path reaches here.
          <p className="disc-media-msg-source" data-testid="media-bubble-source">
            <ImageIcon size={13} aria-hidden="true" />
            <span>{t(`disc.media.startedFrom.${source.mode}`)}</span>
            <button
              type="button"
              className="disc-media-msg-source-open"
              onClick={() => onOpenAsset(source.assetId)}
              data-testid="media-bubble-source-open"
            >
              {t('disc.media.openSourceImage')}
            </button>
          </p>
        )}
        {previewUrl && (
          <div className="disc-media-msg-preview" data-testid="media-bubble-preview">
            {modality === 'video'
              ? <video src={previewUrl} controls aria-label={t('disc.media.playerLabel', prompt)} />
              : <img src={previewUrl} alt={prompt} />}
          </div>
        )}
        {run.status === 'success' && assetId && (
          <button
            type="button"
            className="btn btn-sm disc-media-msg-open"
            onClick={() => onOpenAsset(assetId)}
            data-testid="media-bubble-open-asset"
          >
            <ExternalLink size={14} aria-hidden="true" />
            <span>{t('run.media.open')}</span>
          </button>
        )}
      </article>
    </div>
  );
}
