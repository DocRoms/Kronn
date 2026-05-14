/**
 * 0.8.3 (#288) — fleet-wide active-audits popover.
 *
 * Mirror of `workflows/ActiveRunsPopover` but for audit runs. Rendered
 * from `Dashboard.tsx` when the user clicks the Projets nav button
 * while at least one audit is in progress AND we're not already on
 * the projects page (otherwise normal navigation wins).
 *
 * Lists every project currently auditing with:
 *   - project name + current step file
 *   - live elapsed wall-clock
 *   - Stop button (POST /projects/:id/cancel-audit)
 *   - click anywhere on the row → navigate to the project + scroll
 *
 * Reuses the same CSS classes as ActiveRunsPopover for visual parity.
 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Square, X, ChevronRight } from 'lucide-react';
import type { AuditProgress, Project } from '../types/generated';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
// CSS class names are shared with the workflows popover (visual parity
// + zero CSS duplication). We import the workflows stylesheet directly.
import './workflows/ActiveRunsPopover.css';

export interface ActiveAuditsPopoverProps {
  audits: AuditProgress[];
  projects: Project[];
  onClose: () => void;
  onNavigateToProject: (projectId: string) => void;
  onViewAllProjects: () => void;
  /** Called after a cancel succeeds so the parent can refetch the
   *  fleet-wide audit-status snapshot. */
  onAfterCancel?: () => void;
}

function formatElapsed(ms: number, t: (k: string, ...a: (string | number)[]) => string): string {
  // Defensive: Math.max(0, NaN) === NaN — clamp explicitly so a
  // malformed `started_at` (server clock issue, empty string)
  // surfaces as "0s" instead of "NaNs" in the UI.
  const safe = Number.isFinite(ms) ? ms : 0;
  const s = Math.max(0, Math.floor(safe / 1000));
  if (s < 60) return t('wf.elapsed', `${s}s`);
  const m = Math.floor(s / 60);
  if (m < 60) return t('wf.elapsed', `${m}m ${s % 60}s`);
  const h = Math.floor(m / 60);
  return t('wf.elapsed', `${h}h ${m % 60}m`);
}

export function ActiveAuditsPopover({
  audits,
  projects,
  onClose,
  onNavigateToProject,
  onViewAllProjects,
  onAfterCancel,
}: ActiveAuditsPopoverProps) {
  const { t } = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const [cancellingIds, setCancellingIds] = useState<Set<string>>(new Set());
  const [nowTick, setNowTick] = useState(() => Date.now());

  // Build a project-id → project-name lookup so we don't iterate
  // `projects` per audit. Cheap; recomputes on every projects refetch.
  const projectName = useMemo(() => {
    const m: Record<string, string> = {};
    for (const p of projects) m[p.id] = p.name;
    return m;
  }, [projects]);

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
    // the popover doesn't immediately close it — same race fix as
    // ActiveRunsPopover.
    const tid = window.setTimeout(() => {
      document.addEventListener('mousedown', onMouseDown);
    }, 0);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onMouseDown);
      window.clearTimeout(tid);
    };
  }, [onClose]);

  const handleCancel = async (projectId: string) => {
    setCancellingIds(prev => {
      const next = new Set(prev);
      next.add(projectId);
      return next;
    });
    try {
      await projectsApi.cancelAudit(projectId);
    } catch {
      // Parent's refetch reconciles state; no toast wiring here.
    }
    onAfterCancel?.();
  };

  return (
    <div
      ref={rootRef}
      className="wf-active-runs-popover"
      role="dialog"
      aria-label={t('audit.activeAuditsTitle')}
    >
      <div className="wf-active-runs-header">
        <span className="wf-active-runs-title">{t('audit.activeAuditsTitle')}</span>
        <button
          type="button"
          className="wf-active-runs-close"
          onClick={onClose}
          aria-label="Close"
        >
          <X size={12} />
        </button>
      </div>

      {audits.length === 0 ? (
        <div className="wf-active-runs-empty">{t('audit.activeAuditsEmpty')}</div>
      ) : (
        <ul className="wf-active-runs-list">
          {audits.map(a => {
            const isCancelling = cancellingIds.has(a.project_id);
            const elapsedMs = nowTick - new Date(a.started_at).getTime();
            const name = projectName[a.project_id] ?? a.project_id;
            return (
              <li key={a.project_id} className="wf-active-runs-item">
                <button
                  type="button"
                  className="wf-active-runs-item-body"
                  onClick={() => onNavigateToProject(a.project_id)}
                >
                  <Loader2 size={12} className="spin text-accent" />
                  <div className="wf-active-runs-item-text">
                    <div className="wf-active-runs-item-name">{name}</div>
                    <div className="wf-active-runs-item-meta">
                      <span>{t('audit.activeAuditsStep', a.step_index, a.total_steps)}</span>
                      {a.current_file && (
                        <>
                          <span className="wf-active-runs-item-sep">·</span>
                          <span>{a.current_file}</span>
                        </>
                      )}
                      <span className="wf-active-runs-item-sep">·</span>
                      <span>{formatElapsed(elapsedMs, t)}</span>
                    </div>
                  </div>
                </button>
                <button
                  type="button"
                  className="wf-active-runs-stop-btn"
                  onClick={e => {
                    e.stopPropagation();
                    void handleCancel(a.project_id);
                  }}
                  disabled={isCancelling}
                  title={t('audit.cancelAudit')}
                >
                  <Square size={10} style={{ fill: 'currentColor' }} />
                  {isCancelling ? t('wf.cancelling') : t('audit.cancelAudit')}
                </button>
              </li>
            );
          })}
        </ul>
      )}

      <button
        type="button"
        className="wf-active-runs-footer"
        onClick={onViewAllProjects}
      >
        <span>{t('audit.viewAllProjects')}</span>
        <ChevronRight size={12} />
      </button>
    </div>
  );
}
