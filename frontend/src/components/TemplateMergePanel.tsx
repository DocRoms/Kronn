import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, CheckCircle2, GitMerge, RotateCcw, Trash2, X } from 'lucide-react';
import type { TemplateMergeOffer } from '../hooks/useTemplateMergeOffer';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { countByAction, errorText, MAPPING_ACTIONS, templateMergeStatus } from '../lib/templateMerge';
import { LoadingState } from './LoadingState';
import { GitDiffViewer } from './GitDiffViewer';
import type {
  Correspondence, MappingAction, MergeBase, TemplateMergeAccepted, TemplateMergeState, TemplateMergeView, TemplateMergeWarning,
} from '../types/generated';
import './GitPanel.css';
import './TemplateMergePanel.css';

type Op = 'plan' | 'start' | 'accept' | 'discard';

type Phase =
  | { kind: 'loading'; op: Op }
  | { kind: 'plan'; plan: TemplateMergeView }
  | { kind: 'result'; view: TemplateMergeView; accepted: TemplateMergeAccepted | null }
  | { kind: 'discarded' }
  | { kind: 'error'; op: Op; message: string; retry: () => void };

export interface TemplateMergePanelProps {
  projectId: string;
  /** Opened from "Lancer l'audit": offers to run the audit without merging. */
  gate?: boolean;
  onLaunchAuditAnyway?: () => void;
  onClose?: () => void;
}

function useLabels() {
  const { t } = useT();
  const action: Record<MappingAction, { label: string; hint: string }> = {
    extracted: { label: t('templateMerge.action.extracted'), hint: t('templateMerge.action.extractedHint') },
    target_exists: { label: t('templateMerge.action.targetExists'), hint: t('templateMerge.action.targetExistsHint') },
    unmapped: { label: t('templateMerge.action.unmapped'), hint: t('templateMerge.action.unmappedHint') },
    not_prose: { label: t('templateMerge.action.notProse'), hint: t('templateMerge.action.notProseHint') },
    kronn_managed: { label: t('templateMerge.action.kronnManaged'), hint: t('templateMerge.action.kronnManagedHint') },
  };
  const fix: Record<Op, string> = {
    plan: t('templateMerge.error.fixPlan'),
    start: t('templateMerge.error.fixStart'),
    accept: t('templateMerge.error.fixAccept'),
    discard: t('templateMerge.error.fixDiscard'),
  };
  const loading: Record<Op, string> = {
    plan: t('templateMerge.loading.plan'),
    start: t('templateMerge.loading.start'),
    accept: t('templateMerge.loading.accept'),
    discard: t('templateMerge.loading.discard'),
  };
  return { t, action, fix, loading };
}

function ActionBadge({ action }: { action: MappingAction }) {
  const { action: labels } = useLabels();
  return (
    <span className="tm-badge" data-action={action} title={labels[action].hint}>
      {labels[action].label}
    </span>
  );
}

function lineRange(row: Correspondence): string {
  return row.start_line === 0 ? '—' : `${row.start_line}–${row.end_line}`;
}

function CorrespondenceTable({ plan }: { plan: TemplateMergeView }) {
  const { t, action } = useLabels();
  const counts = countByAction(plan);
  return (
    <>
      <div className="tm-counts" data-testid="template-merge-counts">
        {MAPPING_ACTIONS.filter(a => counts[a] > 0).map(a => (
          <span key={a} className="tm-count">
            <ActionBadge action={a} /> {counts[a]}
          </span>
        ))}
      </div>
      {counts.unmapped > 0 && (
        <p className="tm-unmapped-note" data-testid="template-merge-unmapped-note">
          <AlertTriangle size={12} /> {t('templateMerge.unmappedNote', counts.unmapped)}
        </p>
      )}
      <div className="tm-table-wrap">
        <table className="tm-table" data-testid="template-merge-table">
          <thead>
            <tr>
              <th>{t('templateMerge.table.source')}</th>
              <th>{t('templateMerge.table.section')}</th>
              <th>{t('templateMerge.table.lines')}</th>
              <th>{t('templateMerge.table.target')}</th>
              <th>{t('templateMerge.table.action')}</th>
            </tr>
          </thead>
          <tbody>
            {plan.correspondence.map((row, index) => (
              <tr
                key={`${row.source}:${row.start_line}:${index}`}
                className={row.action === 'unmapped' ? 'tm-row-unmapped' : undefined}
                data-action={row.action}
              >
                <td><code>{row.source}</code></td>
                <td>{row.section ?? <span className="tm-dim">{t('templateMerge.table.wholeFile')}</span>}</td>
                <td className="tm-nowrap">{lineRange(row)}</td>
                <td>{row.target ? <code>{row.target}</code> : <span className="tm-dim">—</span>}</td>
                <td>
                  <ActionBadge action={row.action} />
                  {row.action === 'unmapped' && (
                    <div className="tm-row-explain">{action.unmapped.hint}</div>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

function UntrackedWarning({ paths, onRecheck }: { paths: string[]; onRecheck: () => void }) {
  const { t } = useLabels();
  if (paths.length === 0) return null;
  return (
    <div className="tm-warning" role="alert" data-testid="template-merge-untracked">
      <AlertTriangle size={14} className="tm-warning-icon" />
      <div>
        <strong>{t('templateMerge.untracked.title', paths.length)}</strong>
        <p>{t('templateMerge.untracked.desc')}</p>
        <ul className="tm-path-list">
          {paths.map(p => <li key={p}><code>{p}</code></li>)}
        </ul>
        <button type="button" className="tm-btn" onClick={onRecheck}>
          <RotateCcw size={12} /> {t('templateMerge.untracked.recheck')}
        </button>
      </div>
    </div>
  );
}

/** What the plan itself warns about, e.g. local docs a remote base cannot contain. */
function PlanWarnings({ warnings }: { warnings: TemplateMergeWarning[] }) {
  const { t } = useT();
  if (warnings.length === 0) return null;
  return (
    <div className="tm-warning" role="alert" data-testid="template-merge-warnings">
      <AlertTriangle size={14} className="tm-warning-icon" />
      <div>
        {warnings.map(w => {
          const listed = w.more > 0
            ? `${w.paths.join(', ')} ${t('templateMerge.warning.more', w.more)}`
            : w.paths.join(', ');
          return <p key={w.code}>{t(`templateMerge.warning.${w.code}`, w.paths.length + w.more, listed)}</p>;
        })}
      </div>
    </div>
  );
}

/** Abandon behind a confirmation; `warning` names what else would be lost. */
function DiscardControl({ onDiscard, warning }: { onDiscard: () => void; warning?: string }) {
  const { t } = useLabels();
  const [confirming, setConfirming] = useState(false);
  if (!confirming) {
    return (
      <button type="button" className="tm-btn tm-btn-danger-ghost" onClick={() => setConfirming(true)} data-testid="template-merge-discard">
        <Trash2 size={12} /> {t('templateMerge.discard')}
      </button>
    );
  }
  return (
    <span className="tm-confirm">
      <span>{t('templateMerge.discardConfirmText')}{warning && <> <strong>{warning}</strong></>}</span>
      <button type="button" className="tm-btn tm-btn-danger" onClick={onDiscard} data-testid="template-merge-discard-confirm">
        <Trash2 size={12} /> {t('templateMerge.discardConfirm')}
      </button>
      <button type="button" className="tm-btn" onClick={() => setConfirming(false)}>
        {t('templateMerge.cancel')}
      </button>
    </span>
  );
}

/** A merge already on disk, shown from the plan before any POST. */
function MergeStateBox({ merge, onResume, onDiscard }: {
  merge: TemplateMergeState;
  onResume: () => void;
  onDiscard: () => void;
}) {
  const { t } = useLabels();
  const status = templateMergeStatus(merge);
  if (status === 'none') return null;
  const ahead = merge.branch_ahead_commits;
  if (status === 'to_integrate') {
    return (
      <div className="tm-state" data-status={status} role="status" data-testid="template-merge-to-integrate">
        <CheckCircle2 size={14} className="tm-state-icon" />
        <div>
          <strong>{t('templateMerge.integrate.title', ahead)}</strong>
          <p>{t('templateMerge.integrate.desc', merge.branch, merge.checkout_branch ?? t('templateMerge.integrate.currentBranch'))}</p>
          <div className="tm-actions">
            <button type="button" className="tm-btn" onClick={onResume} data-testid="template-merge-state-resume">
              <RotateCcw size={12} /> {t('templateMerge.resume')}
            </button>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="tm-state" data-status={status} role="status" data-testid="template-merge-in-progress">
      <GitMerge size={14} className="tm-state-icon" />
      <div>
        <strong>{t('templateMerge.progress.title')}</strong>
        <p>{t('templateMerge.progress.desc', merge.pending_files, ahead)}</p>
        {merge.worktree_path && (
          <p className="tm-dim tm-small">{t('templateMerge.result.worktree')} <code>{merge.worktree_path}</code></p>
        )}
        <div className="tm-actions">
          <button type="button" className="tm-btn tm-btn-primary" onClick={onResume} data-testid="template-merge-state-resume">
            <RotateCcw size={12} /> {t('templateMerge.resume')}
          </button>
          <DiscardControl
            onDiscard={onDiscard}
            warning={ahead > 0 ? t('templateMerge.progress.discardLosesCommits', ahead) : undefined}
          />
        </div>
      </div>
    </div>
  );
}

export function TemplateMergePanel({ projectId, gate = false, onLaunchAuditAnyway, onClose }: TemplateMergePanelProps) {
  const { t, fix, loading } = useLabels();
  const [phase, setPhase] = useState<Phase>({ kind: 'loading', op: 'plan' });
  const [base, setBase] = useState<MergeBase>('local');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const busy = useRef(false);

  // One request at a time; every failure keeps a "Reprendre" that replays it.
  const run = useCallback(function runOp<T>(op: Op, call: () => Promise<T>, done: (value: T) => void) {
    if (busy.current) return;
    busy.current = true;
    setPhase({ kind: 'loading', op });
    const retry = () => runOp(op, call, done);
    call().then(
      value => { busy.current = false; done(value); },
      error => { busy.current = false; setPhase({ kind: 'error', op, message: errorText(error), retry }); },
    );
  }, []);

  const loadPlan = useCallback(() => {
    run('plan', () => projectsApi.templateMergePlan(projectId), plan => setPhase({ kind: 'plan', plan }));
  }, [run, projectId]);

  const showResult = useCallback((view: TemplateMergeView, accepted: TemplateMergeAccepted | null) => {
    setSelected(new Set(view.files.map(f => f.path)));
    setPhase({ kind: 'result', view, accepted });
  }, []);

  const start = useCallback((chosen: MergeBase) => {
    run('start', () => projectsApi.templateMergeStart(projectId, chosen), view => showResult(view, null));
  }, [run, projectId, showResult]);

  const accept = useCallback((view: TemplateMergeView, paths: string[]) => {
    if (paths.length === 0) return;
    run('accept', () => projectsApi.templateMergeAccept(projectId, paths), accepted => {
      const committed = new Set(accepted.committed);
      showResult({ ...view, files: view.files.filter(f => !committed.has(f.path)) }, accepted);
    });
  }, [run, projectId, showResult]);

  const discard = useCallback(() => {
    run('discard', () => projectsApi.templateMergeDiscard(projectId), () => setPhase({ kind: 'discarded' }));
  }, [run, projectId]);

  useEffect(() => { loadPlan(); }, [loadPlan]);

  const toggle = (path: string) => setSelected(prev => {
    const next = new Set(prev);
    if (next.has(path)) next.delete(path); else next.add(path);
    return next;
  });

  const resultFiles = phase.kind === 'result' ? phase.view.files : null;
  const selectedPaths = useMemo(
    () => (resultFiles ?? []).map(f => f.path).filter(p => selected.has(p)),
    [resultFiles, selected],
  );

  const waitingForIntegration = phase.kind === 'plan'
    ? phase.plan.merge.branch_ahead_commits > 0
    : phase.kind === 'result' && (phase.accepted !== null || phase.view.merge.branch_ahead_commits > 0);
  const gateInProgress = phase.kind === 'plan' && templateMergeStatus(phase.plan.merge) === 'in_progress';

  return (
    <section className="tm-panel" data-testid="template-merge-panel" aria-label={t('templateMerge.title')}>
      <header className="tm-header">
        <GitMerge size={16} className="tm-header-icon" />
        <div className="tm-header-text">
          <h3>{t('templateMerge.title')}</h3>
          <p>{t('templateMerge.subtitle')}</p>
        </div>
        {onClose && (
          <button type="button" className="tm-icon-btn" onClick={onClose} aria-label={t('templateMerge.close')}>
            <X size={14} />
          </button>
        )}
      </header>

      {gate && (
        <div className="tm-gate" data-testid="template-merge-gate">
          <p>{t(gateInProgress ? 'templateMerge.gate.inProgress' : 'templateMerge.gate.desc')}</p>
          {onLaunchAuditAnyway && !waitingForIntegration && (
            <button type="button" className="tm-btn" onClick={onLaunchAuditAnyway}>
              {t('templateMerge.gate.auditAnyway')}
            </button>
          )}
        </div>
      )}

      {phase.kind === 'loading' && <LoadingState message={loading[phase.op]} />}

      {phase.kind === 'error' && (
        <div className="tm-error" role="alert" data-testid="template-merge-error">
          <AlertTriangle size={14} className="tm-error-icon" />
          <div>
            <strong>{t('templateMerge.error.title')}</strong>
            <pre className="tm-error-message">{phase.message}</pre>
            <p className="tm-error-fix">{fix[phase.op]}</p>
            <button type="button" className="tm-btn tm-btn-primary" onClick={phase.retry}>
              <RotateCcw size={12} /> {t('templateMerge.resume')}
            </button>
          </div>
        </div>
      )}

      {phase.kind === 'plan' && templateMergeStatus(phase.plan.merge) !== 'none' && (
        <>
          <MergeStateBox merge={phase.plan.merge} onResume={() => start(base)} onDiscard={discard} />
          <UntrackedWarning paths={phase.plan.untracked_in_checkout} onRecheck={loadPlan} />
          <details className="tm-details">
            <summary>{t('templateMerge.result.table', phase.plan.correspondence.length)}</summary>
            <CorrespondenceTable plan={phase.plan} />
          </details>
        </>
      )}

      {phase.kind === 'plan' && templateMergeStatus(phase.plan.merge) === 'none' && (
        <>
          <UntrackedWarning paths={phase.plan.untracked_in_checkout} onRecheck={loadPlan} />
          <p className="tm-summary">
            {t('templateMerge.plan.summary', phase.plan.docs_dir, phase.plan.files.length, phase.plan.existing_targets.length)}
          </p>
          <CorrespondenceTable plan={phase.plan} />
          {phase.plan.files.length > 0 && (
            <details className="tm-details">
              <summary>{t('templateMerge.plan.files', phase.plan.files.length)}</summary>
              <ul className="tm-path-list">
                {phase.plan.files.map(f => (
                  <li key={f.path}>
                    <code>{f.path}</code>
                    {f.imported_sections > 0 && <span className="tm-dim"> · {t('templateMerge.importedSections', f.imported_sections)}</span>}
                  </li>
                ))}
              </ul>
            </details>
          )}
          <fieldset className="tm-base">
            <legend>{t('templateMerge.base.legend')}</legend>
            {(['local', 'remote'] as const).map(option => (
              <label key={option} className="tm-base-option">
                <input
                  type="radio"
                  name={`tm-base-${projectId}`}
                  value={option}
                  checked={base === option}
                  onChange={() => setBase(option)}
                />
                <span>
                  <strong>{t(option === 'local' ? 'templateMerge.base.local' : 'templateMerge.base.remote')}</strong>
                  <span className="tm-dim"> — {t(option === 'local' ? 'templateMerge.base.localHint' : 'templateMerge.base.remoteHint')}</span>
                </span>
              </label>
            ))}
          </fieldset>
          <div className="tm-actions">
            <button type="button" className="tm-btn tm-btn-primary" onClick={() => start(base)} data-testid="template-merge-launch">
              <GitMerge size={12} /> {t('templateMerge.launch')}
            </button>
          </div>
        </>
      )}

      {phase.kind === 'result' && (
        <>
          <p className="tm-summary" data-testid="template-merge-result-summary">
            {t('templateMerge.result.summary', phase.view.branch ?? '', phase.view.base_ref)}
            {phase.view.resumed && <span className="tm-dim"> · {t('templateMerge.result.resumed')}</span>}
          </p>
          {phase.view.worktree_path && (
            <p className="tm-dim tm-small">{t('templateMerge.result.worktree')} <code>{phase.view.worktree_path}</code></p>
          )}
          {phase.accepted && (
            <div className="tm-success" role="status" data-testid="template-merge-accepted">
              <CheckCircle2 size={14} />
              <span>
                {t('templateMerge.accepted', phase.accepted.committed.length, phase.accepted.branch, phase.accepted.commit.slice(0, 8))}
                {' '}{t('templateMerge.acceptedNext')}
              </span>
            </div>
          )}
          <UntrackedWarning paths={phase.view.untracked_in_checkout} onRecheck={() => start(base)} />
          <PlanWarnings warnings={phase.view.warnings} />
          <details className="tm-details">
            <summary>{t('templateMerge.result.table', phase.view.correspondence.length)}</summary>
            <CorrespondenceTable plan={phase.view} />
          </details>
          {phase.view.files.length === 0 ? (
            <p className="tm-dim">{t('templateMerge.result.noFiles')}</p>
          ) : (
            <div className="tm-files">
              {phase.view.files.map(file => (
                <div key={file.path} className="tm-file" data-testid="template-merge-file">
                  <label className="tm-file-head">
                    <input
                      type="checkbox"
                      checked={selected.has(file.path)}
                      onChange={() => toggle(file.path)}
                      aria-label={t('templateMerge.result.select', file.path)}
                    />
                    <code>{file.path}</code>
                    {file.outcome === 'already_present' && (
                      <span className="tm-badge" data-action="target_exists">{t('templateMerge.outcome.alreadyPresent')}</span>
                    )}
                    {file.imported_sections > 0 && (
                      <span className="tm-dim">{t('templateMerge.importedSections', file.imported_sections)}</span>
                    )}
                  </label>
                  <GitDiffViewer path={file.path} content={file.diff} loading={false} t={t} />
                </div>
              ))}
            </div>
          )}
          <div className="tm-actions">
            <button
              type="button"
              className="tm-btn tm-btn-primary"
              disabled={selectedPaths.length === 0}
              onClick={() => accept(phase.view, selectedPaths)}
              data-testid="template-merge-accept-selected"
            >
              <CheckCircle2 size={12} /> {t('templateMerge.acceptSelected', selectedPaths.length)}
            </button>
            <button
              type="button"
              className="tm-btn"
              disabled={phase.view.files.length === 0}
              onClick={() => accept(phase.view, phase.view.files.map(f => f.path))}
              data-testid="template-merge-accept-all"
            >
              {t('templateMerge.acceptAll')}
            </button>
            <button type="button" className="tm-btn" onClick={() => start(base)} data-testid="template-merge-resume">
              <RotateCcw size={12} /> {t('templateMerge.resume')}
            </button>
            <DiscardControl onDiscard={discard} />
          </div>
        </>
      )}

      {phase.kind === 'discarded' && (
        <div className="tm-success" role="status" data-testid="template-merge-discarded">
          <CheckCircle2 size={14} />
          <span>{t('templateMerge.discarded')}</span>
          <button type="button" className="tm-btn" onClick={loadPlan}>
            <RotateCcw size={12} /> {t('templateMerge.restart')}
          </button>
        </div>
      )}
    </section>
  );
}

/** The audit section's slot: a banner while the merge is only offered, the panel once opened. */
export function TemplateMergeOfferSlot({ offer, projectId }: { offer: TemplateMergeOffer; projectId: string }) {
  const { t } = useT();
  if (offer.mode !== 'closed') {
    return (
      <TemplateMergePanel
        projectId={projectId}
        gate={offer.mode === 'gate'}
        onLaunchAuditAnyway={offer.mode === 'gate' ? offer.launchAuditAnyway : undefined}
        onClose={offer.close}
      />
    );
  }
  if (offer.status === 'to_integrate' && offer.merge) {
    return (
      <div className="tm-offer" data-testid="template-merge-offer" data-status={offer.status}>
        <CheckCircle2 size={14} className="tm-state-icon" />
        <span>{t('templateMerge.integrate.title', offer.merge.branch_ahead_commits)}</span>
        <button type="button" className="tm-btn" onClick={offer.open}>
          {t('templateMerge.offer.view')}
        </button>
      </div>
    );
  }
  if (!offer.eligible) return null;
  if (offer.status === 'in_progress') {
    return (
      <div className="tm-offer" data-testid="template-merge-offer" data-status={offer.status}>
        <GitMerge size={14} className="tm-header-icon" />
        <span>{t('templateMerge.offer.inProgress')}</span>
        <button type="button" className="tm-btn tm-btn-primary" onClick={offer.open}>
          {t('templateMerge.resume')}
        </button>
      </div>
    );
  }
  return (
    <div className="tm-offer" data-testid="template-merge-offer">
      <GitMerge size={14} className="tm-header-icon" />
      <span>{t('templateMerge.offer.desc')}</span>
      <button type="button" className="tm-btn tm-btn-primary" onClick={offer.open}>
        {t('templateMerge.offer.open')}
      </button>
    </div>
  );
}
