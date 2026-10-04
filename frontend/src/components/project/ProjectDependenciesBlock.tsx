import { useState } from 'react';
import { Package, RefreshCw } from 'lucide-react';
import { projects as projectsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { useCachedResource } from '../../hooks/useCachedResource';
import type { DependencyUpdateSummary } from '../../types/generated';

const INCOMPLETE = ['Unsupported', 'Unavailable', 'Error', 'TimedOut'];

/** The project's dependency freshness: last known result at once, refreshed behind it. */
export function ProjectDependenciesBlock({ projectId, enabled }: { projectId: string; enabled: boolean }) {
  const { t, locale } = useT();
  const [savingMonitoring, setSavingMonitoring] = useState(false);
  const [monitoringError, setMonitoringError] = useState(false);
  const { data, refreshing, error, refresh, set } = useCachedResource<DependencyUpdateSummary>({
    key: enabled ? `dependency-updates:${projectId}` : null,
    load: force => projectsApi.dependencyUpdates(projectId, force),
  });

  const updateMonitoring = async (intervalDays: number | null) => {
    if (savingMonitoring) return;
    setSavingMonitoring(true);
    setMonitoringError(false);
    try {
      await projectsApi.setDependencyMonitoring(projectId, intervalDays);
      set(await projectsApi.dependencyUpdates(projectId));
    } catch {
      setMonitoringError(true);
    } finally {
      setSavingMonitoring(false);
    }
  };

  const formatDate = (iso: string | null | undefined) => iso
    ? new Date(iso).toLocaleString(locale, { dateStyle: 'medium', timeStyle: 'short' })
    : null;
  const checkedAt = formatDate(data?.checked_at);
  const nextCheckAt = formatDate(data?.next_check_at);
  const incomplete = data?.managers.filter(manager => INCOMPLETE.includes(manager.status)).length ?? 0;

  const summary = (() => {
    if (!data) {
      return refreshing
        ? { tone: 'loading', label: t('projects.master.overview.dependenciesChecking') }
        : { tone: 'muted', label: t('projects.master.overview.dependenciesUnavailable') };
    }
    if (data.managers.length === 0) {
      return { tone: 'muted', label: t('projects.master.overview.dependenciesNone') };
    }
    if (data.total_outdated > 0) {
      return { tone: 'warning', label: t('projects.master.overview.dependencyOutdatedCount', data.total_outdated) };
    }
    if (incomplete > 0) {
      return { tone: 'muted', label: t('projects.master.overview.dependenciesPartial', incomplete) };
    }
    return { tone: 'success', label: t('projects.master.overview.dependenciesUpToDate') };
  })();
  const major = !!data?.total_outdated && data.total_major > 0
    ? t('projects.master.overview.dependencyMajorCount', data.total_major)
    : null;

  return (
    <div className="project-overview-dependencies" data-testid="project-overview-dependencies">
      <div className="project-overview-dependencies-head">
        <div className="project-overview-repository-title">
          <span className="project-overview-repository-icon" aria-hidden="true">
            <Package size={17} />
          </span>
          <div>
            <span>{t('projects.master.overview.dependencies')}</span>
            <strong>
              {summary.label}
              {major && <span className="project-overview-dependency-major">{' · '}{major}</span>}
            </strong>
          </div>
        </div>
        <button
          type="button"
          className="project-overview-dependencies-refresh"
          onClick={() => void refresh()}
          disabled={refreshing}
          aria-label={t('projects.master.overview.dependenciesRefresh')}
          title={t('projects.master.overview.dependenciesRefresh')}
        >
          <RefreshCw size={13} className={refreshing ? 'is-spinning' : undefined} />
          {t('projects.master.overview.dependenciesRefresh')}
        </button>
      </div>
      <div className="project-overview-dependencies-meta">
        <span className="project-overview-git-chip" data-tone={summary.tone}>
          <i aria-hidden="true" />
          {summary.label}
          {major && <strong className="project-overview-dependency-major">{' · '}{major}</strong>}
        </span>
        {data?.cached && (
          <span className="project-overview-git-chip">{t('projects.master.overview.dependenciesCached')}</span>
        )}
        {checkedAt && (
          <span className="project-overview-dependencies-date">
            {t('projects.master.overview.dependenciesCheckedAt', checkedAt)}
          </span>
        )}
        {nextCheckAt && (
          <span className="project-overview-dependencies-date">
            {t('projects.master.overview.dependenciesNextCheckAt', nextCheckAt)}
          </span>
        )}
        {data && refreshing && (
          <span className="project-overview-dependencies-date" role="status" data-testid="dependencies-refreshing">
            {t('projects.master.overview.refreshing')}
          </span>
        )}
        {data && (error || monitoringError) && (
          <span className="project-overview-git-chip" data-tone="warning" role="status">
            {t('projects.master.overview.refreshFailed')}
          </span>
        )}
        <label className="project-overview-dependencies-schedule">
          <span>{t('projects.master.overview.dependenciesSchedule')}</span>
          <select
            value={data?.monitoring_interval_days ?? 'manual'}
            disabled={!data || savingMonitoring}
            onChange={event => {
              const value = event.currentTarget.value;
              void updateMonitoring(value === 'manual' ? null : Number(value));
            }}
          >
            <option value="manual">{t('projects.master.overview.dependenciesScheduleManual')}</option>
            <option value="7">{t('projects.master.overview.dependenciesScheduleWeekly')}</option>
            <option value="14">{t('projects.master.overview.dependenciesScheduleFortnightly')}</option>
            <option value="30">{t('projects.master.overview.dependenciesScheduleMonthly')}</option>
          </select>
        </label>
      </div>
      {!!data?.managers.length && (
        <div className="project-overview-dependency-list">
          {data.managers.map(manager => {
            const packages = manager.packages ?? [];
            const status = manager.status === 'UpdatesAvailable'
              ? (
                <>
                  {t('projects.master.overview.dependencyOutdatedCount', manager.outdated)}
                  {manager.major > 0 && (
                    <strong className="project-overview-dependency-major">
                      {' · '}
                      {t('projects.master.overview.dependencyMajorCount', manager.major)}
                    </strong>
                  )}
                </>
              )
              : manager.status === 'UpToDate'
                ? t('projects.master.overview.dependencyUpToDate')
                : manager.status === 'Unsupported'
                  ? t('projects.master.overview.dependencyUnsupported')
                  : manager.status === 'Unavailable'
                    ? t('projects.master.overview.dependencyToolUnavailable')
                    : manager.status === 'TimedOut'
                      ? t('projects.master.overview.dependencyTimedOut')
                      : t('projects.master.overview.dependencyCheckFailed');
            const tone = manager.status === 'UpdatesAvailable'
              ? 'warning'
              : manager.status === 'UpToDate' ? 'success' : 'muted';
            return (
              <div key={`${manager.manager}:${manager.manifest}`} className="project-overview-dependency-row">
                <div>
                  <strong>{manager.manager}</strong>
                  <span>{manager.manifest}</span>
                  {!!packages.length && (
                    <small>
                      {packages.slice(0, 3).map((pkg, index) => (
                        <span
                          key={pkg.name}
                          className={pkg.major ? 'project-overview-dependency-package-major' : undefined}
                        >
                          {index > 0 ? ' · ' : ''}
                          {pkg.name} {pkg.current} → {pkg.latest}
                        </span>
                      ))}
                      {manager.outdated > 3 && <span>{` · +${manager.outdated - 3}`}</span>}
                    </small>
                  )}
                </div>
                <span className="project-overview-git-chip" data-tone={tone}>
                  <i aria-hidden="true" />
                  {status}
                </span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
