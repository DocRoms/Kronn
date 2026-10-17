import { useCallback, useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { CheckCircle2, FileWarning, Loader2, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { lineDiff } from '../lib/lineDiff';
import type { DocProvenanceReport, DocProvenanceStatus, Project, ProvenanceChange } from '../types/generated';
import { useDialogFocus } from '../hooks/useDialogFocus';
import './DocReconfirm.css';

export interface DocReconfirmProps {
  project: Project;
  auditActive: boolean;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  hideTrigger?: boolean;
  onChanged?: () => void;
}

function ChangeView({ change }: { change: ProvenanceChange }) {
  const { t } = useT();
  const title = change.section ? t('docReconfirm.section', change.section) : t('docReconfirm.wholeDoc');
  const diff = change.before !== null && change.after !== null ? lineDiff(change.before, change.after) : null;
  return (
    <div className="dr-change" data-testid="doc-reconfirm-change">
      <div className="dr-change-title">{title}</div>
      {change.before === null && <p className="dr-hint">{t('docReconfirm.beforeUnavailable')}</p>}
      {change.after === null && <p className="dr-hint">{t('docReconfirm.removed')}</p>}
      {diff ? (
        <pre className="dr-diff">
          {diff.map((line, index) => (
            <span key={index} className="dr-line" data-kind={line.kind}>
              {line.kind === 'add' ? '+ ' : line.kind === 'del' ? '- ' : '  '}{line.text}{'\n'}
            </span>
          ))}
        </pre>
      ) : (
        <>
          {change.before !== null && <pre className="dr-diff" data-kind="before">{change.before}</pre>}
          {change.after !== null && <pre className="dr-diff" data-kind="after">{change.after}</pre>}
        </>
      )}
      {change.truncated && <p className="dr-hint">{t('docReconfirm.truncated')}</p>}
    </div>
  );
}

/**
 * « À reconfirmer » (KT-1079): docs a human validated that Kronn changed since.
 * Every Kronn write is applied; this only asks a person to confirm it, with
 * the diff, and never blocks anything.
 */
export function DocReconfirm({
  project, auditActive, toast,
  open: controlledOpen, onOpenChange, hideTrigger = false, onChanged,
}: DocReconfirmProps) {
  const { t } = useT();
  const [count, setCount] = useState(0);
  const [internalOpen, setInternalOpen] = useState(false);
  const [report, setReport] = useState<DocProvenanceReport | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const open = controlledOpen ?? internalOpen;
  const setOpen = useCallback((next: boolean) => {
    if (!next) setReport(null);
    if (controlledOpen === undefined) setInternalOpen(next);
    onOpenChange?.(next);
  }, [controlledOpen, onOpenChange]);

  useEffect(() => {
    if (auditActive) return;
    let live = true;
    projectsApi.docProvenance(project.id).then(
      r => { if (live) setCount(r.to_reconfirm); },
      () => { if (live) setCount(0); },
    );
    return () => { live = false; };
  }, [auditActive, project.id]);

  const panelRef = useDialogFocus<HTMLDivElement>({ active: open, onEscape: () => setOpen(false) });

  useEffect(() => {
    if (!open || auditActive) return;
    let live = true;
    setReport(null);
    projectsApi.docProvenance(project.id, true).then(
      nextReport => { if (live) setReport(nextReport); },
      error => {
        if (!live) return;
        toast(t('docReconfirm.error', String(error instanceof Error ? error.message : error)), 'error');
        setOpen(false);
      },
    );
    return () => { live = false; };
  }, [auditActive, open, project.id, setOpen, t, toast]);

  useEffect(() => {
    if (auditActive && open) setOpen(false);
  }, [auditActive, open, setOpen]);

  const openPanel = () => setOpen(true);

  const confirm = useAsyncGuard(async (docs: string[]) => {
    setBusy(docs.length === 1 ? docs[0] : '*');
    try {
      const after = await projectsApi.confirmDocProvenance(project.id, docs);
      setCount(after.to_reconfirm);
      onChanged?.();
      toast(t('docReconfirm.done', String(docs.length || (report?.to_reconfirm ?? 0))), 'success');
      if (after.to_reconfirm === 0) {
        setOpen(false);
      } else {
        setReport(await projectsApi.docProvenance(project.id, true));
      }
    } catch (e) {
      toast(t('docReconfirm.error', String(e instanceof Error ? e.message : e)), 'error');
    } finally {
      setBusy(null);
    }
  });

  if (auditActive || (count === 0 && !open)) return null;

  const pending = (report?.docs ?? []).filter(d => d.state === 'to_reconfirm');
  const docRow = (doc: DocProvenanceStatus) => (
    <li key={doc.doc} className="dr-doc" data-testid="doc-reconfirm-doc" data-doc={doc.doc}>
      <div className="dr-doc-head">
        <code className="dr-path">{doc.doc}</code>
        <span className="dr-chip">
          {doc.kronn_writes.length > 0
            ? doc.kronn_writes.map(w => t(`docReconfirm.writer.${w.writer}`)).join(', ')
            : t('docReconfirm.writer.outside')}
        </span>
        <button
          type="button"
          className="dr-btn"
          data-testid="doc-reconfirm-confirm"
          disabled={busy !== null}
          onClick={() => void confirm([doc.doc])}
        >
          {busy === doc.doc ? <Loader2 size={11} className="spin" /> : <CheckCircle2 size={11} />}
          {t('docReconfirm.confirm')}
        </button>
      </div>
      <p className="dr-hint">{t('docReconfirm.validatedOn', doc.validated_at.slice(0, 10))}</p>
      {doc.changes.map((change, index) => <ChangeView key={index} change={change} />)}
    </li>
  );

  return (
    <>
      {!hideTrigger && (
        <button
          type="button"
          className="dash-drift-update-btn"
          data-testid="project-doc-reconfirm"
          onClick={(e) => { e.stopPropagation(); openPanel(); }}
          title={t('docReconfirm.hint')}
        >
          <FileWarning size={9} />
          {t('docReconfirm.button', String(count))}
        </button>
      )}
      {open && createPortal(
        <div className="dr-overlay" role="presentation" onClick={(e) => { e.stopPropagation(); setOpen(false); }}>
          <div
            ref={panelRef}
            className="dr-panel"
            role="dialog"
            aria-modal="true"
            aria-label={t('docReconfirm.title')}
            data-testid="doc-reconfirm-panel"
            onClick={(e) => e.stopPropagation()}
          >
            <header className="dr-header">
              <h2>{t('docReconfirm.title')}</h2>
              <button type="button" className="dr-icon-btn" aria-label={t('common.close')} onClick={() => setOpen(false)}>
                <X size={14} />
              </button>
            </header>
            <p className="dr-hint">{t('docReconfirm.intro')}</p>
            {report === null
              ? <p className="dr-hint"><Loader2 size={11} className="spin" /> {t('docReconfirm.loading')}</p>
              : <ul className="dr-docs">{pending.map(docRow)}</ul>}
            {pending.length > 1 && (
              <footer className="dr-footer">
                <button
                  type="button"
                  className="dr-btn dr-btn-primary"
                  data-testid="doc-reconfirm-all"
                  disabled={busy !== null}
                  onClick={() => void confirm([])}
                >
                  {busy === '*' ? <Loader2 size={11} className="spin" /> : <CheckCircle2 size={11} />}
                  {t('docReconfirm.confirmAll', String(pending.length))}
                </button>
              </footer>
            )}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
