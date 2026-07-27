import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  Check,
  GitBranch,
  GitCommitHorizontal,
  GitMerge,
  Loader2,
  RefreshCw,
} from 'lucide-react';
import { projects } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type { GitBranchesResponse } from '../types/generated';

interface ProjectGitPanelProps {
  projectId: string;
  onBranchChanged: (branch: string) => void;
}

function shortRelativeDate(timestamp: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    day: '2-digit',
    month: 'short',
    year: 'numeric',
  }).format(new Date(timestamp * 1000));
}

export function ProjectGitPanel({ projectId, onBranchChanged }: ProjectGitPanelProps) {
  const { t, locale } = useT();
  const [overview, setOverview] = useState<GitBranchesResponse | null>(null);
  const [selectedBranch, setSelectedBranch] = useState('');
  const [loading, setLoading] = useState(true);
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      const next = await projects.gitBranches(projectId);
      setOverview(next);
      setSelectedBranch(next.current_branch);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    const timer = window.setTimeout(() => void load(), 0);
    return () => window.clearTimeout(timer);
  }, [load]);

  const localBranches = useMemo(
    () => overview?.branches.filter(branch => !branch.is_remote) ?? [],
    [overview],
  );
  const remoteBranches = useMemo(
    () => overview?.branches.filter(branch => branch.is_remote) ?? [],
    [overview],
  );

  async function switchBranch() {
    if (!selectedBranch || selectedBranch === overview?.current_branch) return;
    setSwitching(true);
    setError('');
    try {
      const result = await projects.gitSwitchBranch(projectId, selectedBranch);
      await load();
      onBranchChanged(result.branch);
    } catch (switchError) {
      setError(switchError instanceof Error ? switchError.message : String(switchError));
    } finally {
      setSwitching(false);
    }
  }

  if (loading && !overview) {
    return (
      <div className="project-git-state">
        <Loader2 size={17} className="is-spinning" />
        {t('projects.git.loading')}
      </div>
    );
  }

  return (
    <div className="project-git-panel" data-testid="project-git-panel">
      <header className="project-git-header">
        <div>
          <span className="project-git-eyebrow">{t('projects.git.eyebrow')}</span>
          <h3>
            <GitBranch size={18} />
            {overview?.current_branch || t('projects.git.detached')}
          </h3>
          <p>{t('projects.git.description')}</p>
        </div>
        <button
          type="button"
          className="project-git-refresh"
          onClick={() => void load()}
          disabled={loading || switching}
          aria-label={t('projects.git.refresh')}
          title={t('projects.git.refresh')}
        >
          <RefreshCw size={14} className={loading ? 'is-spinning' : undefined} />
        </button>
      </header>

      {error && (
        <div className="project-git-error" role="alert">
          <AlertTriangle size={15} />
          <span>{error}</span>
        </div>
      )}

      <section className="project-git-switcher" aria-label={t('projects.git.switchTitle')}>
        <label htmlFor={`project-git-branch-${projectId}`}>
          <span>{t('projects.git.switchTitle')}</span>
          <select
            id={`project-git-branch-${projectId}`}
            value={selectedBranch}
            onChange={event => setSelectedBranch(event.target.value)}
            disabled={switching}
          >
            {localBranches.length > 0 && (
              <optgroup label={t('projects.git.localBranches')}>
                {localBranches.map(branch => (
                  <option key={branch.ref_name} value={branch.name}>
                    {branch.name}{branch.is_current ? ` — ${t('projects.git.current')}` : ''}
                  </option>
                ))}
              </optgroup>
            )}
            {remoteBranches.length > 0 && (
              <optgroup label={t('projects.git.remoteBranches')}>
                {remoteBranches.map(branch => (
                  <option key={branch.ref_name} value={branch.name}>{branch.name}</option>
                ))}
              </optgroup>
            )}
          </select>
        </label>
        <button
          type="button"
          onClick={() => void switchBranch()}
          disabled={
            switching
            || !selectedBranch
            || selectedBranch === overview?.current_branch
          }
        >
          {switching ? <Loader2 size={14} className="is-spinning" /> : <GitBranch size={14} />}
          {t('projects.git.switch')}
        </button>
        <small>{t('projects.git.safeSwitchHint')}</small>
      </section>

      <div className="project-git-layout">
        <section className="project-git-branches">
          <header>
            <div>
              <span>{t('projects.git.branches')}</span>
              <strong>{overview?.branches.length ?? 0}</strong>
            </div>
            {overview?.default_branch && (
              <span className="project-git-default">
                {t('projects.git.defaultBranch')}: {overview.default_branch}
              </span>
            )}
          </header>
          <div className="project-git-branch-list">
            {overview?.branches.map(branch => (
              <article
                key={branch.ref_name}
                data-current={branch.is_current}
                data-remote={branch.is_remote}
              >
                <span className="project-git-branch-node" aria-hidden="true">
                  {branch.is_current ? <Check size={11} /> : <GitBranch size={11} />}
                </span>
                <div>
                  <strong>{branch.name}</strong>
                  <small>{branch.subject || branch.commit.slice(0, 8)}</small>
                  <span>
                    {branch.author}
                    {branch.committed_at > 0
                      ? ` · ${shortRelativeDate(branch.committed_at, locale)}`
                      : ''}
                  </span>
                </div>
                <div className="project-git-branch-badges">
                  <span>{branch.is_remote ? t('projects.git.remote') : t('projects.git.local')}</span>
                  {branch.ahead > 0 && <span>↑{branch.ahead}</span>}
                  {branch.behind > 0 && <span>↓{branch.behind}</span>}
                </div>
              </article>
            ))}
          </div>
        </section>

        <section className="project-git-graph">
          <header>
            <div>
              <span>{t('projects.git.recentHistory')}</span>
              <strong>{overview?.commits.length ?? 0}</strong>
            </div>
            {overview?.truncated && <span>{t('projects.git.boundedHistory')}</span>}
          </header>
          <div className="project-git-commit-list">
            {overview?.commits.map((commit, index) => (
              <article key={commit.hash}>
                <div className="project-git-commit-rail" aria-hidden="true">
                  <span data-merge={commit.parents.length > 1}>
                    {commit.parents.length > 1
                      ? <GitMerge size={11} />
                      : <GitCommitHorizontal size={11} />}
                  </span>
                  {index < overview.commits.length - 1 && <i />}
                </div>
                <div className="project-git-commit-content">
                  <div className="project-git-commit-title">
                    <strong>{commit.subject}</strong>
                    <code>{commit.short_hash}</code>
                  </div>
                  {commit.refs.length > 0 && (
                    <div className="project-git-commit-refs">
                      {commit.refs.map(ref => <span key={ref}>{ref}</span>)}
                    </div>
                  )}
                  <small>
                    {commit.author}
                    {commit.committed_at > 0
                      ? ` · ${shortRelativeDate(commit.committed_at, locale)}`
                      : ''}
                    {commit.parents.length > 1 ? ` · ${t('projects.git.mergeCommit')}` : ''}
                  </small>
                </div>
              </article>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
