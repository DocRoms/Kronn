import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, CheckCircle2, Clock3, ExternalLink, Loader2, XCircle } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatDurationCompact } from '../lib/kronnToolParser';
import './RunStatusCard.css';

export type RunStatusCardKind = 'quick_prompt' | 'quick_api' | 'quick_exec' | 'workflow';
export type RunStatusCardStatus =
  | 'preflight_failed'
  | 'queued'
  | 'running'
  | 'success'
  | 'partial'
  | 'failed'
  | 'cancelled'
  | 'timeout';

export type RunStatusCardProgress = {
  completed: number;
  total: number;
  currentLabel?: string | null;
};

/** A server-derived run projection. Optional fields deliberately remain absent
 * until measured by the source; consumers must not infer them client-side. */
export type RunStatusCardModel = {
  id: string;
  kind: RunStatusCardKind;
  status: RunStatusCardStatus;
  startedAt?: string | null;
  finishedAt?: string | null;
  durationMs?: number | null;
  progress?: RunStatusCardProgress | null;
  result?: unknown;
  diagnostic?: string | null;
  freshness?: 'live' | 'rehydrated' | 'unavailable';
  href?: string | null;
};

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

export function RunStatusCard({ model, compact = false }: { model: RunStatusCardModel; compact?: boolean }) {
  const { t } = useT();
  const [now, setNow] = useState(() => Date.now());
  const active = isActive(model.status);

  useEffect(() => {
    if (!active || !model.startedAt) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active, model.startedAt]);

  const duration = measuredDuration(model, now);
  const progress = model.progress;
  const progressPercent = progress && progress.total > 0
    ? Math.min(100, Math.max(0, (progress.completed / progress.total) * 100))
    : null;
  const result = useMemo(() => resultText(model.result), [model.result]);

  return (
    <section className="run-status-card" data-status={model.status} data-kind={model.kind} data-testid="run-status-card">
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
          {model.diagnostic && <p className="run-status-card-diagnostic">{model.diagnostic}</p>}
          {result && <pre className="run-status-card-result">{result}</pre>}
        </>
      )}
    </section>
  );
}

export function workflowRunStatusCardModel(run: {
  id: string;
  status: string;
  started_at: string;
  finished_at: string | null;
  step_results: Array<{ status: string; step_name: string }>;
}): RunStatusCardModel {
  const statuses: Record<string, RunStatusCardStatus> = {
    Pending: 'queued', Running: 'running', Success: 'success', Partial: 'partial',
    Failed: 'failed', Cancelled: 'cancelled', StoppedByGuard: 'timeout',
    Interrupted: 'failed', WaitingApproval: 'running',
  };
  const completed = run.step_results.filter(step => !['Pending', 'Running', 'WaitingApproval'].includes(step.status)).length;
  const current = run.step_results.find(step => ['Pending', 'Running', 'WaitingApproval'].includes(step.status));
  return {
    id: run.id,
    kind: 'workflow',
    status: statuses[run.status] ?? 'failed',
    startedAt: run.started_at,
    finishedAt: run.finished_at,
    progress: run.step_results.length > 0 ? { completed, total: run.step_results.length, currentLabel: current?.step_name } : null,
    freshness: 'rehydrated',
  };
}
