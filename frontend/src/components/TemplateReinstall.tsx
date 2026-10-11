import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';
import { AlertTriangle, CheckCircle2, RotateCcw, Wrench } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { errorText } from '../lib/templateMerge';
import { LoadingState } from './LoadingState';
import { useDialogFocus } from '../hooks/useDialogFocus';
import type { TemplateReinstallPlan, TemplateReinstallReport } from '../types/generated';
import './TemplateMergePanel.css';

type Op = 'plan' | 'run';

type Phase =
  | { kind: 'idle' }
  | { kind: 'loading'; op: Op }
  | { kind: 'confirm'; plan: TemplateReinstallPlan }
  | { kind: 'done'; report: TemplateReinstallReport }
  | { kind: 'error'; op: Op; message: string; retry: () => void };

/**
 * "Recréer les fichiers du template": reinstalls the template's own files
 * behind a confirmation that lists what is replaced. Every replaced file is
 * backed up by the server first; every failure keeps a "Reprendre" (KT-1169).
 */
export function TemplateReinstall({ projectId }: { projectId: string }) {
  const { t } = useT();
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' });
  const busy = useRef(false);

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
    run('plan', () => projectsApi.templateReinstallPlan(projectId), plan => setPhase({ kind: 'confirm', plan }));
  }, [run, projectId]);

  const reinstall = useCallback((paths: string[]) => {
    run('run', () => projectsApi.templateReinstall(projectId, paths), report => setPhase({ kind: 'done', report }));
  }, [run, projectId]);

  const close = () => setPhase({ kind: 'idle' });
  const openRef = useRef<HTMLButtonElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const shown = useRef(phase.kind);

  // The control that held focus (Confirm, Close) was unmounted: hand focus to the next step's control.
  useEffect(() => {
    const previous = shown.current;
    shown.current = phase.kind;
    const lost = document.activeElement === null || document.activeElement === document.body;
    if (previous === phase.kind || !lost) return;
    if (phase.kind === 'idle') openRef.current?.focus();
    else if (phase.kind === 'done' || phase.kind === 'error') closeRef.current?.focus();
  }, [phase.kind]);

  if (phase.kind === 'idle') {
    return (
      <div className="tm-offer" data-testid="template-reinstall-offer">
        <Wrench size={14} className="tm-header-icon" />
        <span>{t('templateReinstall.hint')}</span>
        <button type="button" className="tm-btn" onClick={loadPlan} data-testid="template-reinstall-open" ref={openRef}>
          {t('templateReinstall.open')}
        </button>
      </div>
    );
  }

  return (
    <section className="tm-panel" data-testid="template-reinstall-panel" aria-label={t('templateReinstall.title')}>
      {phase.kind === 'loading' && (
        <LoadingState message={t(phase.op === 'plan' ? 'templateReinstall.loading.plan' : 'templateReinstall.loading.run')} />
      )}

      {phase.kind === 'error' && (
        <div className="tm-error" role="alert" data-testid="template-reinstall-error">
          <AlertTriangle size={14} className="tm-error-icon" />
          <div>
            <strong>{t('templateReinstall.error.title')}</strong>
            <pre className="tm-error-message">{phase.message}</pre>
            <p className="tm-error-fix">{t(phase.op === 'plan' ? 'templateReinstall.error.fixPlan' : 'templateReinstall.error.fixRun')}</p>
            <div className="tm-actions">
              <button type="button" className="tm-btn tm-btn-primary" onClick={phase.retry}>
                <RotateCcw size={12} /> {t('templateMerge.resume')}
              </button>
              <button type="button" className="tm-btn" onClick={close} ref={closeRef}>{t('templateMerge.close')}</button>
            </div>
          </div>
        </div>
      )}

      {phase.kind === 'confirm' && <ConfirmBody plan={phase.plan} onConfirm={reinstall} onCancel={close} returnFocus={openRef} />}

      {phase.kind === 'done' && (
        <div className="tm-success" role="status" data-testid="template-reinstall-done">
          <CheckCircle2 size={14} />
          <div>
            <strong>{t('templateReinstall.done.title', phase.report.replaced.length, phase.report.created.length)}</strong>
            {phase.report.backup_dir && (
              <p className="tm-small">{t('templateReinstall.done.backups')} <code>{phase.report.backup_dir}</code></p>
            )}
            {phase.report.replaced.length > 0 && (
              <ul className="tm-path-list" data-testid="template-reinstall-replaced">
                {phase.report.replaced.map(file => (
                  <li key={file.path}><code>{file.path}</code> <span className="tm-dim">→ <code>{file.backup}</code></span></li>
                ))}
              </ul>
            )}
            {phase.report.skipped.length > 0 && (
              <p className="tm-small" data-testid="template-reinstall-skipped">{t('templateReinstall.done.skipped', phase.report.skipped.length)}</p>
            )}
            {phase.report.to_reconfirm.length > 0 && (
              <p className="tm-small" data-testid="template-reinstall-reconfirm">{t('templateReinstall.done.reconfirm', phase.report.to_reconfirm.join(', '))}</p>
            )}
            <button type="button" className="tm-btn" onClick={close} ref={closeRef}>{t('templateMerge.close')}</button>
          </div>
        </div>
      )}
    </section>
  );
}

function ConfirmBody({ plan, onConfirm, onCancel, returnFocus }: {
  plan: TemplateReinstallPlan;
  onConfirm: (paths: string[]) => void;
  onCancel: () => void;
  returnFocus: RefObject<HTMLButtonElement | null>;
}) {
  const { t } = useT();
  // Inline in the project card, so non-modal: Tab may leave it and only Escape pressed inside closes it.
  // The open button is re-rendered on cancel, hence returnFocus.
  const dialogRef = useDialogFocus<HTMLDivElement>({ onEscape: onCancel, returnFocus, trap: false });
  const damaged = plan.files.filter(f => f.action === 'replace' && f.kind !== 'filled');
  const filled = plan.files.filter(f => f.action === 'replace' && f.kind === 'filled');
  const created = plan.files.filter(f => f.action === 'create');
  // Damaged files are repaired by default; audited content is an explicit opt-in.
  const [selected, setSelected] = useState<Set<string>>(() => new Set(damaged.map(f => f.path)));
  const toggle = (path: string) => setSelected(prev => {
    const next = new Set(prev);
    if (next.has(path)) next.delete(path); else next.add(path);
    return next;
  });
  const nothingToDo = damaged.length === 0 && filled.length === 0 && created.length === 0;
  const count = selected.size + created.length;
  const row = (f: TemplateReinstallPlan['files'][number]) => (
    <li key={f.path}>
      <label>
        <input
          type="checkbox"
          checked={selected.has(f.path)}
          onChange={() => toggle(f.path)}
          aria-label={f.path}
        />{' '}
        <code>{f.path}</code>
      </label>
      {f.human_sections.length > 0 ? (
        <span className="tm-dim" data-testid="template-reinstall-notice"> · {t('templateReinstall.reconfirmHuman', f.human_sections.join(', '))}</span>
      ) : f.human_validated && (
        <span className="tm-dim" data-testid="template-reinstall-notice"> · {t('templateReinstall.reconfirmValidated')}</span>
      )}
    </li>
  );
  return (
    <div ref={dialogRef} data-testid="template-reinstall-confirm" role="dialog" aria-label={t('templateReinstall.title')}>
      <header className="tm-header">
        <Wrench size={16} className="tm-header-icon" />
        <div className="tm-header-text">
          <h3>{t('templateReinstall.title')}</h3>
          <p>{t('templateReinstall.subtitle')}</p>
        </div>
      </header>
      {plan.audit_running && (
        <div className="tm-warning" role="alert" data-testid="template-reinstall-audit-running">
          <AlertTriangle size={14} className="tm-warning-icon" />
          <span>{t('templateReinstall.auditRunning')}</span>
        </div>
      )}
      {nothingToDo ? (
        <p className="tm-summary" data-testid="template-reinstall-uptodate">{t('templateReinstall.upToDate')}</p>
      ) : (
        <>
          {damaged.length > 0 && (
            <>
              <p className="tm-summary"><strong>{t('templateReinstall.replaceTitle', damaged.length)}</strong> {t('templateReinstall.replaceDesc')}</p>
              <ul className="tm-path-list" data-testid="template-reinstall-replace-list">{damaged.map(row)}</ul>
            </>
          )}
          {filled.length > 0 && (
            <div className="tm-warning" data-testid="template-reinstall-filled">
              <AlertTriangle size={14} className="tm-warning-icon" />
              <div>
                <strong>{t('templateReinstall.filledTitle', filled.length)}</strong>
                <p data-testid="template-reinstall-filled-warning">{t('templateReinstall.filledWarning')}</p>
                <ul className="tm-path-list" data-testid="template-reinstall-filled-list">{filled.map(row)}</ul>
              </div>
            </div>
          )}
          {created.length > 0 && (
            <p className="tm-small tm-dim" data-testid="template-reinstall-create-count">{t('templateReinstall.createCount', created.length)}</p>
          )}
        </>
      )}
      <div className="tm-actions">
        {!nothingToDo && (
          <button
            type="button"
            className="tm-btn tm-btn-danger"
            onClick={() => onConfirm([...selected])}
            disabled={plan.audit_running || count === 0}
            data-testid="template-reinstall-confirm-button"
          >
            <Wrench size={12} /> {t('templateReinstall.confirm', count)}
          </button>
        )}
        <button type="button" className="tm-btn" onClick={onCancel}>{t('templateMerge.cancel')}</button>
      </div>
    </div>
  );
}
