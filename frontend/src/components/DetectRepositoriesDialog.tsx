import { useEffect, useState } from 'react';
import { EyeOff, Loader2, X } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { userError } from '../lib/userError';
import type { DetectedRepo } from '../types/generated';
import './DetectRepositoriesDialog.css';

export interface DetectRepositoriesDialogProps {
  onClose: () => void;
  /** At least one project was created: the host refreshes its list. */
  onAdded: () => void;
  toast: (message: string, type: 'success' | 'error' | 'warning' | 'info') => void;
}

/**
 * "Detect repositories" on the Projects page (KT-1684). It used to be the global
 * Scan button, which created a project for every repository found without
 * asking. Now the repositories found are listed first, all checked, and only the
 * ones left checked are added; "Hide" keeps one from coming back.
 */
export function DetectRepositoriesDialog({ onClose, onAdded, toast }: DetectRepositoriesDialogProps) {
  const { t } = useT();
  const [repos, setRepos] = useState<DetectedRepo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [kept, setKept] = useState<ReadonlySet<string>>(new Set());
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    let alive = true;
    projectsApi.scan().then(found => {
      if (!alive) return;
      // Already a project, or hidden (a dot-directory, or by the owner): not offered.
      const offered = found.filter(repo => !repo.has_project && !repo.hidden);
      setRepos(offered);
      setKept(new Set(offered.map(repo => repo.path)));
    }, e => {
      if (alive) setError(userError(e, t('projects.scanError')));
    });
    return () => { alive = false; };
    // The scan runs once, when the dialog opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggle = (path: string) => setKept(current => {
    const next = new Set(current);
    if (next.has(path)) next.delete(path); else next.add(path);
    return next;
  });

  const hide = async (repo: DetectedRepo) => {
    try {
      await projectsApi.hideScanned(repo.path);
      setRepos(current => (current ?? []).filter(candidate => candidate.path !== repo.path));
      setKept(current => {
        const next = new Set(current);
        next.delete(repo.path);
        return next;
      });
    } catch (e) {
      toast(userError(e), 'error');
    }
  };

  const add = async () => {
    if (adding || !repos) return;
    setAdding(true);
    let added = 0;
    try {
      for (const repo of repos.filter(candidate => kept.has(candidate.path))) {
        await projectsApi.create(repo);
        added += 1;
      }
      toast(t('projects.detect.added', added), 'success');
      onAdded();
      onClose();
    } catch (e) {
      toast(userError(e), 'error');
      // What was created before the failure is on the page: the list refreshes.
      if (added > 0) onAdded();
      setAdding(false);
    }
  };

  const keptCount = repos ? repos.filter(repo => kept.has(repo.path)).length : 0;

  return (
    <div className="dash-modal-overlay" onClick={() => !adding && onClose()}>
      <div
        className="dash-modal detect-repos"
        role="dialog"
        aria-modal="true"
        aria-labelledby="detect-repos-title"
        data-testid="detect-repos"
        onClick={e => e.stopPropagation()}
        onKeyDown={e => { if (e.key === 'Escape' && !adding) onClose(); }}
      >
        <div className="dash-modal-header">
          <h3 id="detect-repos-title" className="dash-modal-title">{t('projects.detect.title')}</h3>
          <button type="button" onClick={onClose} disabled={adding} className="dash-modal-close" aria-label={t('common.close')}>
            <X size={16} />
          </button>
        </div>

        {repos === null && !error && (
          <p className="detect-repos-state" role="status">
            <Loader2 size={14} className="spin" aria-hidden="true" /> {t('projects.scanning')}
          </p>
        )}
        {error && <p className="detect-repos-state detect-repos-error" role="alert">{error}</p>}
        {repos !== null && repos.length === 0 && (
          <p className="detect-repos-state" data-testid="detect-repos-empty">{t('projects.scanNone')}</p>
        )}

        {repos !== null && repos.length > 0 && (
          <ul className="detect-repos-list" aria-label={t('projects.detect.listLabel')}>
            {repos.map(repo => (
              <li key={repo.path} className="detect-repos-row" data-testid="detect-repos-row">
                <label className="detect-repos-label">
                  <input
                    type="checkbox"
                    checked={kept.has(repo.path)}
                    disabled={adding}
                    onChange={() => toggle(repo.path)}
                    aria-label={repo.name}
                  />
                  <span className="detect-repos-text">
                    <span className="detect-repos-name">{repo.name}</span>
                    <span className="detect-repos-path">{repo.path}</span>
                  </span>
                </label>
                <button
                  type="button"
                  className="detect-repos-hide"
                  data-testid="detect-repos-hide"
                  disabled={adding}
                  title={t('projects.detect.hideHint')}
                  aria-label={t('projects.detect.hideOne', repo.name)}
                  onClick={() => void hide(repo)}
                >
                  <EyeOff size={12} aria-hidden="true" />
                  {t('projects.detect.hide')}
                </button>
              </li>
            ))}
          </ul>
        )}

        <div className="detect-repos-footer">
          <button type="button" className="btn btn-ghost" onClick={onClose} disabled={adding}>
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            data-testid="detect-repos-add"
            disabled={adding || keptCount === 0}
            onClick={() => void add()}
          >
            {adding ? <Loader2 size={12} className="spin" aria-hidden="true" /> : null}
            {t('projects.detect.add', keptCount)}
          </button>
        </div>
      </div>
    </div>
  );
}
