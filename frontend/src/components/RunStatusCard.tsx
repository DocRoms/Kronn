import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, Clock3, ExternalLink, Loader2, XCircle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatDurationCompact } from '../lib/kronnToolParser';
import { runsApi } from '../lib/api';
import { useWebSocket } from '../hooks/useWebSocket';
import { mediaRunDetails } from '../lib/mediaRunResult';
import {
  sharedRunStatusCardModel,
  type RunStatusCardModel,
  type RunStatusCardStatus,
} from '../lib/runStatusCardModel';
import './RunStatusCard.css';

function isActive(status: RunStatusCardStatus): boolean {
  return status === 'queued' || status === 'running';
}

function statusIcon(status: RunStatusCardStatus) {
  if (status === 'running' || status === 'queued') return <Loader2 className="spin" size={15} aria-hidden />;
  if (status === 'success') return <CheckCircle2 size={15} aria-hidden />;
  if (status === 'partial' || status === 'preflight_failed' || status === 'timeout') return <AlertTriangle size={15} aria-hidden />;
  return <XCircle size={15} aria-hidden />;
}

function measuredDuration(model: RunStatusCardModel, now: number): number | null {
  if (typeof model.durationMs === 'number' && model.durationMs >= 0) return model.durationMs;
  if (!model.startedAt) return null;
  const start = Date.parse(model.startedAt);
  if (Number.isNaN(start)) return null;
  if (model.finishedAt) {
    const end = Date.parse(model.finishedAt);
    return Number.isNaN(end) ? null : Math.max(0, end - start);
  }
  return isActive(model.status) ? Math.max(0, now - start) : null;
}

function resultText(result: unknown): string | null {
  if (result == null) return null;
  if (typeof result === 'string') return result;
  try {
    return JSON.stringify(result, null, 2);
  } catch {
    return null;
  }
}

export function RunStatusCard({ model: initialModel, runId, compact = false }: { model?: RunStatusCardModel; runId?: string; compact?: boolean }) {
  const { t } = useT();
  const rootRef = useRef<HTMLElement>(null);
  // Start suspended: without a real IntersectionObserver measurement yet, a
  // freshly-mounted card must not assume it is on-screen (DoD #6/#7) — a
  // long timeline mounting N cards would otherwise fire N fetches/sockets
  // before the first layout pass. Only environments without IO support
  // (rare/legacy) fall back to always-visible.
  const [visible, setVisible] = useState(() => typeof IntersectionObserver === 'undefined');
  const [hydrated, setHydrated] = useState<RunStatusCardModel | null>(null);
  const model = hydrated ?? initialModel;
  const [now, setNow] = useState(() => Date.now());
  const active = model ? isActive(model.status) : false;

  useEffect(() => {
    const node = rootRef.current;
    if (!node || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(entries => setVisible(entries[0]?.isIntersecting ?? false), { rootMargin: '200px' });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const hydrate = useCallback(async (freshness: RunStatusCardModel['freshness']) => {
    if (!runId || !visible) return;
    try { setHydrated(sharedRunStatusCardModel(await runsApi.get(runId), freshness)); }
    catch { setHydrated(current => current ? { ...current, freshness: 'unavailable' } : null); }
  }, [runId, visible]);
  useEffect(() => { void hydrate('rehydrated'); }, [hydrate]);
  useWebSocket(message => {
    if (visible && runId && message.type === 'shared_run_updated' && message.run_id === runId) void hydrate('live');
  }, () => { if (visible && runId) void hydrate('rehydrated'); }, Boolean(runId && visible && active));

  useEffect(() => {
    if (!visible || !active || !model?.startedAt) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active, model?.startedAt, visible]);

  const result = useMemo(() => resultText(model?.result), [model?.result]);

  if (!model) return <section ref={rootRef} className="run-status-card" data-testid="run-status-card"><span>{t('run.freshness.unavailable')}</span></section>;

  const duration = measuredDuration(model, now);
  const progress = model.progress;
  // Media details only when the run actually carries them: a malformed or
  // absent result degrades to none, never to zeros.
  const media = model.kind === 'media' ? mediaRunDetails(model.result) : null;
  const progressPercent = progress && progress.total > 0
    ? Math.min(100, Math.max(0, (progress.completed / progress.total) * 100))
    : null;

  return (
    <section ref={rootRef} className="run-status-card" data-status={model.status} data-kind={model.kind} data-testid="run-status-card">
      <div className="run-status-card-header">
        <span className="run-status-card-kind">{t(`run.kind.${model.kind}`)}</span>
        <span className="run-status-card-status" data-status={model.status}>
          {statusIcon(model.status)} {t(`run.status.${model.status}`)}
        </span>
        {model.href && (
          <a className="run-status-card-link" href={model.href} aria-label={t('run.open')}>
            <ExternalLink size={14} aria-hidden />
          </a>
        )}
      </div>
      {!compact && (
        <>
          <div className="run-status-card-meta">
            <span><Clock3 size={13} aria-hidden /> {duration == null ? t('run.durationUnavailable') : formatDurationCompact(duration)}</span>
            {model.freshness && <span data-freshness={model.freshness}>{t(`run.freshness.${model.freshness}`)}</span>}
          </div>
          {progress && progressPercent != null && (
            <div className="run-status-card-progress">
              <div className="run-status-card-progress-label">
                <span>{t('run.progress', progress.completed, progress.total)}</span>
                {progress.currentLabel && <span>{progress.currentLabel}</span>}
              </div>
              <div className="run-status-card-progress-track" role="progressbar" aria-valuenow={progress.completed} aria-valuemax={progress.total}>
                <span style={{ width: `${progressPercent}%` }} />
              </div>
            </div>
          )}
          {media && (
            <div className="run-status-card-media" data-testid="run-status-card-media" data-modality={media.modality}>
              <span>{t(`run.media.${media.modality}`)}</span>
              {media.width && media.height && (
                // Real geometry from the produced file: the provider does not
                // honour the requested resolution.
                <span data-testid="run-status-card-media-size">{media.width}×{media.height}</span>
              )}
              {media.durationMs && <span>{Math.round(media.durationMs / 1000)}s</span>}
              {media.costUsd != null && (
                <span data-testid="run-status-card-media-cost">
                  {media.isByok ? t('run.media.byok') : `$${media.costUsd.toFixed(4)}`}
                </span>
              )}
            </div>
          )}
          {model.diagnostic && <p className="run-status-card-diagnostic">{model.diagnostic}</p>}
          {result && <pre className="run-status-card-result">{result}</pre>}
        </>
      )}
    </section>
  );
}
