// KT-549 — one durable media bubble at the launch's transcript position.
//
// This component owns the whole message row. Rendering a normal user bubble
// next to a run card made one generation look like two unrelated messages and
// made the card disappear on success. The root stays stable for every state;
// the completed asset opens through the canonical Assets viewer.
import { ExternalLink, Sparkles } from 'lucide-react';
import { RunStatusCard } from './RunStatusCard';
import { useT } from '../lib/I18nContext';
import { mediaRunDetails } from '../lib/mediaRunResult';
import { sharedRunStatusCardModel } from '../lib/runStatusCardModel';
import type { SharedRun } from '../types/generated';

export function InlineMediaJob({
  messageId,
  prompt,
  run,
  onOpenAsset,
}: {
  messageId: string;
  prompt: string;
  run: SharedRun;
  onOpenAsset: (assetId: string) => void;
}) {
  const { t } = useT();
  const assetId = mediaRunDetails(run.result)?.assetId;
  const model = { ...sharedRunStatusCardModel(run, 'live'), href: null };
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
        <RunStatusCard model={model} runId={run.id} />
        {run.status === 'success' && assetId && (
          <button
            type="button"
            className="btn btn-sm disc-media-msg-open"
            onClick={() => onOpenAsset(assetId)}
            data-testid="media-bubble-open-asset"
          >
            <ExternalLink size={14} aria-hidden="true" />
            <span>{t('run.open')}</span>
          </button>
        )}
      </article>
    </div>
  );
}
