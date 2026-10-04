import { GitBranch, ExternalLink, GitPullRequest, Tag, RefreshCw } from 'lucide-react';
import { projects as projectsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { projectGitCacheKey, useCachedResource } from '../../hooks/useCachedResource';
import type { GitStatusResponse } from '../../types/generated';

const LANGUAGE_COLORS: Record<string, string> = {
  TypeScript: '#3178c6',
  JavaScript: '#f1e05a',
  Rust: '#dea584',
  PHP: '#4f5d95',
  Python: '#3572a5',
  Go: '#00add8',
  Java: '#b07219',
  Kotlin: '#a97bff',
  Swift: '#f05138',
  C: '#555555',
  'C++': '#f34b7d',
  'C#': '#178600',
  Ruby: '#701516',
  Vue: '#41b883',
  Svelte: '#ff3e00',
  CSS: '#663399',
  HTML: '#e34c26',
  Shell: '#89e051',
};

/** The project's repository state: local Git status at once, the PR link and the
 *  language bar completing behind it, the last result kept while it refreshes. */
export function ProjectGitBlock({
  projectId,
  repoUrl,
  enabled,
}: {
  projectId: string;
  repoUrl: string | null;
  enabled: boolean;
}) {
  const { t, locale } = useT();
  const { data: git, fetchedAt, refreshing, error, refresh } = useCachedResource<GitStatusResponse>({
    key: enabled ? projectGitCacheKey(projectId) : null,
    load: async (force, publish) => {
      if (force) return projectsApi.gitStatus(projectId, true);
      publish(await projectsApi.gitStatus(projectId, false, undefined, undefined, true));
      
      return projectsApi.gitStatus(projectId);
    },
  });
  const refreshLanguages = () => void refresh();

  const repositoryUrl = git?.remote_url
    ?? (repoUrl?.startsWith('http') ? repoUrl.replace(/\.git\/?$/, '') : null);
  const pullRequestsUrl = git?.pull_requests_url ?? git?.pr_url ?? null;
  const languageStats = git?.languages ?? [];
  const languageTotalBytes = languageStats.reduce((total, item) => total + item.bytes, 0);
  const timeOf = (iso: string | number) => new Date(iso).toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' });
  const languageCheckedTime = git?.languages_checked_at ? timeOf(git.languages_checked_at) : null;
  const checkedTime = fetchedAt ? timeOf(fetchedAt) : null;

  const gitSync = (() => {
    if (!git) {
      return refreshing
        ? { tone: 'loading', label: t('projects.master.overview.gitLoading') }
        : { tone: 'muted', label: t('projects.master.overview.gitUnavailable') };
    }
    if (!git.has_upstream) {
      return {
        tone: repositoryUrl ? 'warning' : 'muted',
        label: repositoryUrl
          ? t('projects.master.overview.noUpstream')
          : t('projects.master.overview.localOnly'),
      };
    }
    if (git.ahead > 0 && git.behind > 0) {
      return { tone: 'warning', label: t('projects.master.overview.diverged', git.ahead, git.behind) };
    }
    if (git.behind > 0) return { tone: 'warning', label: t('projects.master.overview.behind', git.behind) };
    if (git.ahead > 0) return { tone: 'info', label: t('projects.master.overview.ahead', git.ahead) };
    return { tone: 'success', label: t('projects.master.overview.upToDate') };
  })();

  return (
    <div className="project-overview-repository" data-testid="project-overview-repository">
      <div className="project-overview-repository-head">
        <div className="project-overview-repository-title">
          <span className="project-overview-repository-icon" aria-hidden="true">
            <GitBranch size={17} />
          </span>
          <div>
            <span>{t('projects.master.overview.repository')}</span>
            {repositoryUrl ? (
              <a href={repositoryUrl} target="_blank" rel="noreferrer">
                {repositoryUrl.replace(/^https?:\/\//, '')}
                <ExternalLink size={11} />
              </a>
            ) : (
              <strong>{t('projects.master.overview.local')}</strong>
            )}
          </div>
        </div>
        <div className="project-overview-repository-actions">
          {repositoryUrl && (
            <a href={repositoryUrl} target="_blank" rel="noreferrer">
              <ExternalLink size={13} />
              {t('projects.master.overview.openRepository')}
            </a>
          )}
          {pullRequestsUrl && (
            <a href={pullRequestsUrl} target="_blank" rel="noreferrer">
              <GitPullRequest size={13} />
              {git?.provider === 'gitlab'
                ? t('projects.master.overview.mergeRequests')
                : t('projects.master.overview.pullRequests')}
            </a>
          )}
        </div>
      </div>
      <div className="project-overview-repository-meta">
        <span className="project-overview-git-chip">
          <GitBranch size={12} />
          {git?.branch || t('projects.master.overview.unknownBranch')}
        </span>
        <span className="project-overview-git-chip">
          <Tag size={12} />
          {git?.last_tag || t('projects.master.overview.noTag')}
        </span>
        <span className="project-overview-git-chip" data-tone={gitSync.tone}>
          <i aria-hidden="true" />
          {gitSync.label}
        </span>
        {git && checkedTime && (
          <span className="project-overview-dependencies-date">
            {t('projects.master.overview.gitCheckedAt', checkedTime)}
          </span>
        )}
        {git && refreshing && (
          <span className="project-overview-dependencies-date" role="status" data-testid="git-refreshing">
            {t('projects.master.overview.refreshing')}
          </span>
        )}
        {git && error && (
          <span className="project-overview-git-chip" data-tone="warning" role="status">
            {t('projects.master.overview.refreshFailed')}
          </span>
        )}
        {!!git?.files.length && (
          <span className="project-overview-git-chip" data-tone="warning">
            {t('projects.master.overview.localChanges', git.files.length)}
          </span>
        )}
        {languageCheckedTime && (
          <button
            type="button"
            className="project-overview-language-refresh"
            data-cached={git?.languages_cached}
            onClick={() => void refreshLanguages()}
            disabled={refreshing}
            aria-label={t('projects.master.overview.languagesRefresh')}
            title={git?.languages_cached
              ? t('projects.master.overview.languagesCachedAt', languageCheckedTime)
              : t('projects.master.overview.languagesCheckedAt', languageCheckedTime)}
          >
            <RefreshCw size={11} className={refreshing ? 'is-spinning' : undefined} />
            {git?.languages_cached
              ? t('projects.master.overview.languagesCachedShort', languageCheckedTime)
              : languageCheckedTime}
          </button>
        )}
      </div>
      {/* KT-94 follow-up — the bar now arrives ~20 s AFTER the git
          status (background computation). Rendering nothing until then
          made the whole card jump when it landed; keep the slot at its
          final height with a pending shimmer instead (CLS ≈ 0). */}
      {languageStats.length === 0 && (
        <div className="project-overview-languages" aria-hidden="true">
          <div className="project-overview-languages-title">
            <strong>{t('projects.master.overview.languages')}</strong>
            <span>{t('projects.master.overview.languagesPending')}</span>
          </div>
          <div className="project-overview-language-bar project-overview-language-bar--pending" />
          <div className="project-overview-language-legend">
            <span>
              <i style={{ background: 'var(--kr-text-faint)' }} />
              <strong>…</strong>
            </span>
          </div>
        </div>
      )}
      {languageStats.length > 0 && languageTotalBytes > 0 && (
        <div className="project-overview-languages">
          <div className="project-overview-languages-title">
            <strong>{t('projects.master.overview.languages')}</strong>
            <span>{t('projects.master.overview.languagesHint')}</span>
          </div>
          <div
            className="project-overview-language-bar"
            role="img"
            aria-label={t('projects.master.overview.languages')}
          >
            {languageStats.map(item => {
              const percentage = (item.bytes / languageTotalBytes) * 100;
              return (
                <span
                  key={item.language}
                  style={{
                    width: `${percentage}%`,
                    background: LANGUAGE_COLORS[item.language] ?? 'var(--kr-text-faint)',
                  }}
                  title={`${item.language} · ${percentage.toFixed(1)} %`}
                />
              );
            })}
          </div>
          <div className="project-overview-language-legend">
            {languageStats.slice(0, 8).map(item => {
              const percentage = (item.bytes / languageTotalBytes) * 100;
              return (
                <span key={item.language}>
                  <i style={{ background: LANGUAGE_COLORS[item.language] ?? 'var(--kr-text-faint)' }} />
                  <strong>{item.language}</strong>
                  {percentage.toFixed(1)} %
                </span>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
