/**
 * KT-999 — "Existing documentation?" in the briefing: the locations Kronn
 * found in the repository, pre-checked, plus paths typed by hand. Each kept
 * location gets a closed state, a short note and, at most one per kind, the
 * source-of-truth mark. The server re-validates everything on save.
 */
import { useEffect, useState } from 'react';
import { Plus } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { projects as projectsApi } from '../lib/api';
import type { BriefingDocKind, BriefingDocLocation, BriefingDocState } from '../types/generated';
import {
  BRIEFING_DOC_STATES, DOC_NOTE_MAX, DOC_PATH_MAX, rowsToLocations, setSourceOfTruth, toRow, type DocRow,
} from './briefingDocs';

const kindKey = (kind: BriefingDocKind) => `briefing.docs.kind.${kind}`;

interface Props {
  projectId: string;
  onChange: (locations: BriefingDocLocation[]) => void;
}

export function BriefingExistingDocs({ projectId, onChange }: Props) {
  const { t } = useT();
  const [rows, setRows] = useState<DocRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [manual, setManual] = useState('');
  const [manualError, setManualError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    let cancelled = false;
    projectsApi.briefingDocs(projectId)
      .then(found => { if (!cancelled) setRows(found.map(toRow)); })
      .catch(e => console.warn('[BriefingExistingDocs] detection failed:', e))
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [projectId]);

  useEffect(() => { onChange(rowsToLocations(rows)); }, [rows, onChange]);

  const patch = (path: string, change: Partial<DocRow>) =>
    setRows(rs => rs.map(r => (r.path === path ? { ...r, ...change } : r)));

  const chooseState = (path: string, state: BriefingDocState) =>
    setRows(rs => rs.map(r => (r.path === path
      ? { ...r, state, source_of_truth: state === 'obsolete' ? false : r.source_of_truth }
      : r)));

  const addManual = async () => {
    const typed = manual.trim();
    if (!typed || checking) return;
    setChecking(true);
    setManualError(null);
    try {
      const doc = await projectsApi.checkBriefingDoc(projectId, typed);
      setRows(rs => (rs.some(r => r.path === doc.path)
        ? rs.map(r => (r.path === doc.path ? { ...r, included: true } : r))
        : [...rs, toRow(doc)]));
      setManual('');
    } catch (e) {
      setManualError((e as Error).message || String(e));
    } finally {
      setChecking(false);
    }
  };

  return (
    <section className="briefing-project briefing-docs" aria-labelledby="briefing-docs-title" data-testid="briefing-docs">
      <h4 id="briefing-docs-title" className="briefing-form-q">{t('briefing.docs.title')}</h4>
      <p className="briefing-form-intro">{t('briefing.docs.intro')}</p>
      {loading && <p className="briefing-form-intro">{t('briefing.docs.detecting')}</p>}
      {!loading && rows.length === 0 && <p className="briefing-form-intro">{t('briefing.docs.none')}</p>}

      {rows.map(row => (
        <fieldset key={row.path} className="briefing-card briefing-doc" data-doc-path={row.path}>
          <legend className="briefing-doc-head">
            <label className="briefing-doc-include">
              <input
                type="checkbox"
                checked={row.included}
                onChange={e => patch(row.path, { included: e.target.checked })}
                aria-label={t('briefing.docs.keep', row.path)}
              />
              <code>{row.path}{row.is_dir ? '/' : ''}</code>
            </label>
            <span className="briefing-doc-kind">{t(kindKey(row.kind))}</span>
            <span className="briefing-doc-date">
              {row.last_commit ? t('briefing.docs.lastCommit', row.last_commit) : t('briefing.docs.noCommit')}
            </span>
          </legend>
          {row.included && (
            <>
              <div className="briefing-card-options" role="radiogroup" aria-label={t('briefing.docs.stateQ', row.path)}>
                {BRIEFING_DOC_STATES.map(state => {
                  const selected = row.state === state;
                  return (
                    <button
                      key={state}
                      type="button"
                      role="radio"
                      aria-checked={selected}
                      className={`briefing-card-option${selected ? ' is-selected' : ''}`}
                      onClick={() => chooseState(row.path, state)}
                    >
                      {t(`briefing.docs.state.${state}`)}
                    </button>
                  );
                })}
              </div>
              <div className="briefing-doc-extra">
                <input
                  className="briefing-card-input"
                  aria-label={t('briefing.docs.noteLabel', row.path)}
                  placeholder={t('briefing.docs.notePlaceholder')}
                  value={row.note}
                  maxLength={DOC_NOTE_MAX}
                  onChange={e => patch(row.path, { note: e.target.value })}
                />
                <label className="briefing-doc-sot">
                  <input
                    type="checkbox"
                    checked={row.source_of_truth}
                    disabled={row.state === 'obsolete'}
                    onChange={e => setRows(rs => setSourceOfTruth(rs, row.path, e.target.checked))}
                    aria-label={t('briefing.docs.sourceOfTruthFor', row.path)}
                  />
                  <span>{t('briefing.docs.sourceOfTruth')}</span>
                </label>
              </div>
            </>
          )}
        </fieldset>
      ))}

      <div className="briefing-doc-manual">
        <input
          className="briefing-card-input"
          aria-label={t('briefing.docs.manualLabel')}
          placeholder={t('briefing.docs.manualPlaceholder')}
          value={manual}
          maxLength={DOC_PATH_MAX}
          onChange={e => { setManual(e.target.value); setManualError(null); }}
          onKeyDown={e => {
            if (e.key === 'Enter') {
              e.preventDefault();
              void addManual();
            }
          }}
        />
        <button
          type="button"
          className="briefing-card-option"
          onClick={() => void addManual()}
          disabled={checking || !manual.trim()}
          data-testid="briefing-docs-add"
        >
          <Plus size={11} /> {t('briefing.docs.add')}
        </button>
      </div>
      {manualError && <p className="briefing-doc-error" role="alert">{manualError}</p>}
      <p className="briefing-form-intro">{t('briefing.docs.sourceOfTruthHint')}</p>
    </section>
  );
}
