import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Square, X, ChevronRight } from 'lucide-react';
import type { WorkflowSummary } from '../../types/generated';
import { workflows as workflowsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';

export interface ActiveRunsPopoverProps {
  workflows: WorkflowSummary[];
  onClose: () => void;
  onNavigateToWorkflow: (workflowId: string) => void;
  onViewAllWorkflows: () => void;
  onAfterCancel?: () => void;
}

function formatElapsed(ms: number, t: (k: string, ...a: (string | number)[]) => string): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return t('wf.elapsed', `${s}s`);
  const m = Math.floor(s / 60);
  if (m < 60) return t('wf.elapsed', `${m}m ${s % 60}s`);
  const h = Math.floor(m / 60);
  return t('wf.elapsed', `${h}h ${m % 60}m`);
}

export function ActiveRunsPopover({
  workflows,
  onClose,
  onNavigateToWorkflow,
  onViewAllWorkflows,
  onAfterCancel,
}: ActiveRunsPopoverProps) {
  const { t } = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const [cancellingIds, setCancellingIds] = useState<Set<string>>(new Set());
  const [nowTick, setNowTick] = useState(() => Date.now());

  const activeRuns = useMemo(
    () => workflows.filter(w =>
      w.last_run && (w.last_run.status === 'Running' || w.last_run.status === 'Pending'),
    ),
    [workflows],
  );

  useEffect(() => {
    const id = setInterval(() => setNowTick(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    const onMouseDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener('keydown', onKey);
    // Defer the outside-click listener one tick so the click that opened
    // the popover doesn't immediately close it.
    const tid = window.setTimeout(() => {
      document.addEventListener('mousedown', onMouseDown);
    }, 0);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onMouseDown);
      window.clearTimeout(tid);
    };
  }, [onClose]);

  const handleCancel = async (workflowId: string, runId: string) => {
    setCancellingIds(prev => {
      const next = new Set(prev);
      next.add(runId);
      return next;
    });
    try {
      await workflowsApi.cancelRun(workflowId, runId);
    } catch {
      // Parent refetch reconciles state; no toast system here.
    }
    onAfterCancel?.();
  };

  return (
    <div
      ref={rootRef}
      className="wf-active-runs-popover"
      role="dialog"
      aria-label={t('wf.activeRunsTitle')}
    >
      <div className="wf-active-runs-header">
        <span className="wf-active-runs-title">{t('wf.activeRunsTitle')}</span>
        <button
          type="button"
          className="wf-active-runs-close"
          onClick={onClose}
          aria-label="Close"
        >
          <X size={12} />
        </button>
      </div>

      {activeRuns.length === 0 ? (
        <div className="wf-active-runs-empty">{t('wf.activeRunsEmpty')}</div>
      ) : (
        <ul className="wf-active-runs-list">
          {activeRuns.map(wf => {
            const run = wf.last_run!;
            const isCancelling = cancellingIds.has(run.id);
            const elapsedMs = nowTick - new Date(run.started_at).getTime();
            return (
              <li key={run.id} className="wf-active-runs-item">
                <button
                  type="button"
                  className="wf-active-runs-item-body"
                  onClick={() => onNavigateToWorkflow(wf.id)}
                >
                  <Loader2 size={12} className="spin text-accent" />
                  <div className="wf-active-runs-item-text">
                    <div className="wf-active-runs-item-name">{wf.name}</div>
                    <div className="wf-active-runs-item-meta">
                      {wf.project_name && (
                        <>
                          <span>{wf.project_name}</span>
                          <span className="wf-active-runs-item-sep">·</span>
                        </>
                      )}
                      <span>{formatElapsed(elapsedMs, t)}</span>
                    </div>
                  </div>
                </button>
                <button
                  type="button"
                  className="wf-active-runs-stop-btn"
                  onClick={e => {
                    e.stopPropagation();
                    void handleCancel(wf.id, run.id);
                  }}
                  disabled={isCancelling}
                  title={t('wf.cancelRun')}
                >
                  <Square size={10} style={{ fill: 'currentColor' }} />
                  {isCancelling ? t('wf.cancelling') : t('wf.cancelRun')}
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <button
        type="button"
        className="wf-active-runs-footer"
        onClick={onViewAllWorkflows}
      >
        <span>{t('wf.viewAllWorkflows')}</span>
        <ChevronRight size={12} />
      </button>
    </div>
  );
}
