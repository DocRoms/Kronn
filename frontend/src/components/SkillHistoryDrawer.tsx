/**
 * KT-1434 — version history of a custom skill, the counterpart of the Quick
 * Prompt drawer (same `qph-*` chrome): every saved change is a version, two
 * neighbours can be compared line by line, and an old one can be restored,
 * which records it as the newest version.
 */
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { useDialogFocus } from '../hooks/useDialogFocus';
import { useT } from '../lib/I18nContext';
import { skills as skillsApi } from '../lib/api';
import type { SkillVersion } from '../types/generated';
import { diffLines } from '../lib/qp-history-diff';
import { History, X, ChevronRight, ChevronDown, RotateCcw, CheckCircle2 } from 'lucide-react';
import './QPHistoryDrawer.css';

interface Props {
  skillId: string;
  skillName: string;
  /** Changes whenever the skill is saved: the history is read again. */
  refreshKey: string;
  /** Called after a restore, so the page reloads the skill. */
  onRestored?: () => void;
}

interface Load {
  key: string;
  history: SkillVersion[];
  error: string | null;
}

const EMPTY: SkillVersion[] = [];

function fmtDate(iso: string, locale: string): string {
  try {
    return new Date(iso).toLocaleString(locale, {
      day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

export default function SkillHistoryDrawer({ skillId, skillName, refreshKey, onRestored }: Props) {
  const { t, locale } = useT();
  const [open, setOpen] = useState(false);
  const [load, setLoad] = useState<Load | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [diffOpen, setDiffOpen] = useState<number | null>(null);
  const [restoring, setRestoring] = useState<number | null>(null);
  const [count, setCount] = useState<number | null>(null);
  const [restored, setRestored] = useState<string | null>(null);
  const [reloads, setReloads] = useState(0);
  const key = `${skillId}:${refreshKey}:${reloads}`;
  const dialogRef = useDialogFocus<HTMLElement>({ active: open, onEscape: () => setOpen(false) });
  const current = open && load?.key === key;
  const history = current ? load.history : EMPTY;
  const loading = open && !current;
  const error = actionError ?? (current ? load.error : null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const hist = await skillsApi.history(skillId);
        if (cancelled) return;
        setCount((hist ?? []).length);
        setLoad({ key, history: hist ?? [], error: null });
      } catch (e) {
        if (cancelled) return;
        setCount(0);
        setLoad({ key, history: [], error: String((e as Error).message ?? e) });
      }
    })();
    return () => { cancelled = true; };
  }, [skillId, key]);

  const restore = useAsyncGuard(async (index: number) => {
    if (!window.confirm(t('skills.history.restoreConfirm', index))) return;
    setRestoring(index);
    setActionError(null);
    setRestored(null);
    try {
      const created = await skillsApi.restoreVersion(skillId, index);
      setExpanded(null);
      setDiffOpen(null);
      setRestored(t('skills.history.restored', index, created?.version_index ?? '?'));
      // The drawer reads its own history again; the page reloads the skill.
      setReloads(n => n + 1);
      onRestored?.();
    } catch (e) {
      setActionError(String((e as Error).message ?? e));
    } finally {
      setRestoring(null);
    }
  });

  return (
    <>
      <button
        type="button"
        className="qph-cta"
        data-testid="skill-history-toggle"
        onClick={() => { setActionError(null); setRestored(null); setOpen(true); }}
        title={t('skills.history.openTitle', skillName)}
      >
        <History size={12} />
        {t('skills.history.openLabel', count ?? '…')}
      </button>

      {/* Portaled: a transformed ancestor (sheet header) would clip a fixed drawer. */}
      {open && createPortal(
        <>
          <div className="qph-overlay" onClick={() => setOpen(false)} />
          <aside
            ref={dialogRef}
            className="qph-drawer"
            data-testid="skill-history-drawer"
            role="dialog"
            aria-modal="true"
            aria-label={t('skills.history.drawerTitle', skillName)}
          >
            <header className="qph-drawer-header">
              <h2 className="qph-drawer-title">
                {t('skills.history.drawerTitle', skillName)}
                <span className="qph-drawer-title-count">({history.length})</span>
              </h2>
              <button
                type="button"
                className="qph-drawer-close"
                onClick={() => setOpen(false)}
                aria-label={t('common.close')}
              >
                <X size={16} />
              </button>
            </header>

            <div className="qph-drawer-body">
              {loading && <p>{t('common.loading')}</p>}
              {error && <p className="qph-error" role="alert">{error}</p>}
              {restored && (
                <p className="qph-restored" role="status" data-testid="skill-history-restored">
                  <CheckCircle2 size={12} aria-hidden /> {restored}
                </p>
              )}
              {!loading && !error && history.length === 0 && (
                <p className="qph-empty">{t('skills.history.empty')}</p>
              )}

              {history.length > 0 && (
                <div className="qph-versions">
                  {history.map((v, idx) => {
                    const isOpen = expanded === v.version_index;
                    const prev = history[idx + 1];
                    return (
                      <div
                        key={v.version_index}
                        className="qph-version-card"
                        data-active={isOpen ? 'true' : 'false'}
                        data-current={v.current ? 'true' : 'false'}
                        data-testid={`skill-history-version-${v.version_index}`}
                      >
                        <div className="qph-version-card-header-row">
                          <button
                            type="button"
                            className="qph-version-card-header"
                            onClick={() => setExpanded(isOpen ? null : v.version_index)}
                            aria-expanded={isOpen}
                          >
                            {isOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                            <span className="qph-version-tag">v{v.version_index}</span>
                            {v.current && (
                              <span className="qph-version-current-pill">{t('skills.history.current')}</span>
                            )}
                            <span className="qph-version-date">{fmtDate(v.created_at, locale)}</span>
                          </button>
                          {!v.current && (
                            <button
                              type="button"
                              className="qph-version-restore"
                              data-testid={`skill-history-restore-${v.version_index}`}
                              onClick={(e) => { e.stopPropagation(); void restore(v.version_index); }}
                              disabled={restoring != null}
                              aria-label={t('skills.history.restoreAria', v.version_index)}
                              title={t('skills.history.restoreAria', v.version_index)}
                            >
                              <RotateCcw size={11} aria-hidden /> {t('skills.history.restore')}
                            </button>
                          )}
                        </div>

                        {isOpen && (
                          <div className="qph-version-body">
                            <p className="qph-version-description">
                              {v.icon} {v.name}{v.description ? ` — ${v.description}` : ''}
                            </p>
                            {prev && (
                              <div className="qph-version-template-actions">
                                <button
                                  type="button"
                                  className="qph-diff-toggle"
                                  data-testid={`skill-history-diff-toggle-${v.version_index}`}
                                  onClick={() => setDiffOpen(diffOpen === v.version_index ? null : v.version_index)}
                                >
                                  {diffOpen === v.version_index
                                    ? t('qp.history.hideDiff')
                                    : t('qp.history.showDiff', prev.version_index, v.version_index)}
                                </button>
                              </div>
                            )}
                            {diffOpen === v.version_index && prev ? (
                              <div className="qph-diff" data-testid={`skill-history-diff-${v.version_index}`}>
                                <div className="qph-diff-col qph-diff-col-prev">
                                  <header>{t('qp.history.diffPrev', prev.version_index)}</header>
                                  {diffLines(prev.content, v.content).map((row, i) => (
                                    <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.prev || ' '}</div>
                                  ))}
                                </div>
                                <div className="qph-diff-col qph-diff-col-next">
                                  <header>{t('qp.history.diffNext', v.version_index)}</header>
                                  {diffLines(prev.content, v.content).map((row, i) => (
                                    <div key={i} className={`qph-diff-line qph-diff-line-${row.kind}`}>{row.next || ' '}</div>
                                  ))}
                                </div>
                              </div>
                            ) : (
                              <pre className="qph-template-preview">{v.content}</pre>
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </aside>
        </>,
        document.body,
      )}
    </>
  );
}
