import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle,
  Box,
  CircleCheck,
  CircleHelp,
  Container,
  Copy,
  ExternalLink,
  FileCode2,
  Loader2,
  Play,
  RefreshCw,
  RotateCw,
  ScrollText,
  Square,
  X,
} from 'lucide-react';
import { projects } from '../lib/api';
import { useT } from '../lib/I18nContext';
import type {
  ProjectDockerAction,
  ProjectDockerEndpoint,
  ProjectDockerLogs,
  ProjectDockerStatus,
} from '../types/generated';
import './ProjectDockerPanel.css';

interface ProjectDockerPanelProps {
  projectId: string;
  toast: (message: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onOpenConfig: (path: string) => void;
  onRunningChange?: (running: boolean) => void;
}

type BusyAction = `${ProjectDockerAction}:${string}`;

export function ProjectDockerPanel({
  projectId,
  toast,
  onOpenConfig,
  onRunningChange,
}: ProjectDockerPanelProps) {
  const { t, locale } = useT();
  const [status, setStatus] = useState<ProjectDockerStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [busyAction, setBusyAction] = useState<BusyAction | null>(null);
  const [logs, setLogs] = useState<ProjectDockerLogs | null>(null);
  const [logsService, setLogsService] = useState<string | null>(null);
  const [logsLoading, setLogsLoading] = useState(false);
  const [logsError, setLogsError] = useState('');
  const actionInFlight = useRef(false);

  const applyStatus = useCallback((next: ProjectDockerStatus) => {
    setStatus(next);
    onRunningChange?.(next.services.some(service => service.running));
  }, [onRunningChange]);

  const load = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      applyStatus(await projects.dockerStatus(projectId));
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setLoading(false);
    }
  }, [applyStatus, projectId]);

  useEffect(() => {
    const timer = window.setTimeout(() => void load(), 0);
    return () => window.clearTimeout(timer);
  }, [load]);

  useEffect(() => {
    if (!logsService) return undefined;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setLogsService(null);
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [logsService]);

  const runningCount = useMemo(
    () => status?.services.filter(service => service.running).length ?? 0,
    [status],
  );

  const endpointStatus = (endpoint: ProjectDockerEndpoint) => {
    switch (endpoint.host_status) {
      case 'configured':
        return { label: t('projects.docker.hostConfigured'), Icon: CircleCheck };
      case 'missing':
        return { label: t('projects.docker.hostMissing'), Icon: AlertTriangle };
      case 'non_local':
        return { label: t('projects.docker.hostNonLocal'), Icon: AlertTriangle };
      default:
        return { label: t('projects.docker.hostUnknown'), Icon: CircleHelp };
    }
  };

  const endpointLink = (endpoint: ProjectDockerEndpoint) => {
    const { label, Icon } = endpointStatus(endpoint);
    return (
      <a
        key={endpoint.url}
        href={endpoint.url}
        target="_blank"
        rel="noreferrer"
        data-host-status={endpoint.host_status}
        title={`${t('projects.docker.openEndpoint', endpoint.url)} · ${label}`}
      >
        <Icon size={12} aria-label={label} />
        <span>{endpoint.url}</span>
        <ExternalLink size={11} aria-hidden="true" />
      </a>
    );
  };

  const loadLogs = useCallback(async (service: string) => {
    setLogsService(service);
    setLogsLoading(true);
    setLogsError('');
    setLogs(null);
    try {
      setLogs(await projects.dockerLogs(projectId, service));
    } catch (loadError) {
      setLogs(null);
      setLogsError(loadError instanceof Error ? loadError.message : String(loadError));
    } finally {
      setLogsLoading(false);
    }
  }, [projectId]);

  const copyLogs = useCallback(async () => {
    if (!logs?.output) return;
    try {
      await navigator.clipboard.writeText(logs.output);
      toast(t('projects.docker.logsCopied'), 'success');
    } catch {
      toast(t('projects.docker.logsCopyFailed'), 'error');
    }
  }, [logs, t, toast]);

  const runAction = useCallback(async (action: ProjectDockerAction, service?: string) => {
    if (actionInFlight.current) return;
    actionInFlight.current = true;
    const target = service ?? 'all';
    setBusyAction(`${action}:${target}`);
    setError('');
    try {
      const next = await projects.dockerAction(projectId, action, service);
      applyStatus(next);
      toast(t('projects.docker.actionSuccess', t(`projects.docker.${action}`), service ?? t('projects.docker.allServices')), 'success');
    } catch (actionError) {
      const message = actionError instanceof Error ? actionError.message : String(actionError);
      setError(message);
      toast(t('projects.docker.actionFailed', message), 'error');
    } finally {
      actionInFlight.current = false;
      setBusyAction(null);
    }
  }, [applyStatus, projectId, t, toast]);

  const actionButton = (
    action: ProjectDockerAction,
    service: string | undefined,
    label: string,
    Icon: typeof Play,
    className?: string,
  ) => {
    const key = `${action}:${service ?? 'all'}` as BusyAction;
    const pending = busyAction === key;
    return (
      <button
        type="button"
        className={className}
        onClick={() => void runAction(action, service)}
        disabled={busyAction !== null || loading}
        title={label}
      >
        {pending ? <Loader2 size={14} className="is-spinning" /> : <Icon size={14} />}
        <span>{label}</span>
      </button>
    );
  };

  if (loading && !status) {
    return (
      <div className="project-docker-state">
        <Loader2 size={18} className="is-spinning" />
        {t('projects.docker.loading')}
      </div>
    );
  }

  return (
    <div className="project-docker-panel" data-testid="project-docker-panel">
      <header className="project-docker-header">
        <div>
          <span className="project-docker-eyebrow">{t('projects.docker.eyebrow')}</span>
          <h3><Container size={19} /> {t('projects.docker.title')}</h3>
          <p>{t('projects.docker.description')}</p>
        </div>
        <div className="project-docker-header-actions">
          {status?.compose_file && (
            <button
              type="button"
              className="project-docker-open-config"
              onClick={() => {
                if (status.compose_file) onOpenConfig(status.compose_file);
              }}
            >
              <FileCode2 size={14} />
              <span>{t('projects.docker.openConfig')}</span>
            </button>
          )}
          <button
            type="button"
            className="project-docker-refresh"
            onClick={() => void load()}
            disabled={loading || busyAction !== null}
            aria-label={t('projects.docker.refresh')}
            title={t('projects.docker.refresh')}
          >
            <RefreshCw size={14} className={loading ? 'is-spinning' : undefined} />
          </button>
        </div>
      </header>

      {error && (
        <div className="project-docker-error" role="alert">
          <AlertTriangle size={15} />
          <span>{error}</span>
        </div>
      )}

      {!status?.compose_present ? (
        <div className="project-docker-empty">
          <span><Box size={22} /></span>
          <strong>{t('projects.docker.noComposeTitle')}</strong>
          <p>{t('projects.docker.noComposeDescription')}</p>
        </div>
      ) : !status.docker_available || !status.daemon_available ? (
        <div className="project-docker-empty project-docker-unavailable">
          <span><AlertTriangle size={22} /></span>
          <strong>{t('projects.docker.unavailableTitle')}</strong>
          <p>{status.error || t('projects.docker.unavailableDescription')}</p>
          <button type="button" onClick={() => void load()} disabled={loading}>
            <RefreshCw size={14} /> {t('projects.docker.retry')}
          </button>
        </div>
      ) : (
        <>
          <section className="project-docker-toolbar">
            <div>
              <strong>{t('projects.docker.services')}</strong>
              <span>{t('projects.docker.runningCount', runningCount, status.services.length)}</span>
              {status.compose_file && <code>{status.compose_file}</code>}
            </div>
            <div className="project-docker-global-actions">
              {actionButton('start', undefined, t('projects.docker.startAll'), Play, 'is-primary')}
              {actionButton('stop', undefined, t('projects.docker.stopAll'), Square)}
              {actionButton('restart', undefined, t('projects.docker.restartAll'), RotateCw)}
            </div>
          </section>

          {status.services.length === 0 ? (
            <div className="project-docker-empty is-compact">
              <strong>{t('projects.docker.noServices')}</strong>
            </div>
          ) : (
            <div className="project-docker-services">
              {status.services.map(service => (
                <article
                  key={`${service.service}:${service.container_name ?? service.state}`}
                  data-running={service.running}
                >
                  <div className="project-docker-service-main">
                    <span className="project-docker-state-dot" aria-hidden="true" />
                    <div>
                      <h4>{service.service}</h4>
                      <p>{service.status || (service.state === 'not_created'
                        ? t('projects.docker.notCreated')
                        : service.state)}</p>
                    </div>
                  </div>
                  <div className="project-docker-service-meta">
                    {(service.container_name || service.image) && (
                      <span>
                        <Box size={12} />
                        {service.container_name || service.image}
                        {service.container_name && service.image && <small>{service.image}</small>}
                      </span>
                    )}
                    {service.health && (
                      <span className="project-docker-health">
                        {t('projects.docker.health')}: <strong>{service.health}</strong>
                      </span>
                    )}
                    <span className="project-docker-ports">
                      {service.ports.length > 0 ? service.ports.join(' · ') : t('projects.docker.noPorts')}
                    </span>
                    {service.endpoints.length > 0 && (
                      <div className="project-docker-endpoints">
                        {service.endpoints.slice(0, 3).map(endpointLink)}
                        {service.endpoints.length > 3 && (
                          <details>
                            <summary>{t('projects.docker.moreEndpoints', service.endpoints.length - 3)}</summary>
                            <div>{service.endpoints.slice(3).map(endpointLink)}</div>
                          </details>
                        )}
                      </div>
                    )}
                  </div>
                  <div className="project-docker-service-actions">
                    {service.running && (
                      <button
                        type="button"
                        onClick={() => void loadLogs(service.service)}
                        disabled={busyAction !== null}
                        title={t('projects.docker.openLogs')}
                      >
                        <ScrollText size={14} />
                        <span>{t('projects.docker.logs')}</span>
                      </button>
                    )}
                    {service.running
                      ? actionButton('stop', service.service, t('projects.docker.stop'), Square)
                      : actionButton('start', service.service, t('projects.docker.start'), Play, 'is-primary')}
                    {service.running && actionButton('restart', service.service, t('projects.docker.restart'), RotateCw)}
                  </div>
                </article>
              ))}
            </div>
          )}

          <footer className="project-docker-footer">
            {t('projects.docker.checkedAt', new Intl.DateTimeFormat(locale, {
              dateStyle: 'short',
              timeStyle: 'medium',
            }).format(new Date(status.checked_at)))}
          </footer>
        </>
      )}

      {logsService && (
        <div
          className="project-docker-logs-overlay"
          role="presentation"
          onMouseDown={event => {
            if (event.target === event.currentTarget) setLogsService(null);
          }}
        >
          <section
            className="project-docker-logs-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="project-docker-logs-title"
          >
            <header>
              <div>
                <span>{t('projects.docker.logs')}</span>
                <h3 id="project-docker-logs-title"><ScrollText size={17} /> {logsService}</h3>
              </div>
              <div>
                <button
                  type="button"
                  onClick={() => void loadLogs(logsService)}
                  disabled={logsLoading}
                  title={t('projects.docker.refreshLogs')}
                  aria-label={t('projects.docker.refreshLogs')}
                >
                  <RefreshCw size={14} className={logsLoading ? 'is-spinning' : undefined} />
                </button>
                <button
                  type="button"
                  onClick={() => void copyLogs()}
                  disabled={!logs?.output}
                  title={t('projects.docker.copyLogs')}
                  aria-label={t('projects.docker.copyLogs')}
                >
                  <Copy size={14} />
                </button>
                <button
                  type="button"
                  onClick={() => setLogsService(null)}
                  title={t('common.close')}
                  aria-label={t('common.close')}
                >
                  <X size={15} />
                </button>
              </div>
            </header>
            <div className="project-docker-logs-content">
              {logsLoading && !logs ? (
                <div className="project-docker-logs-state"><Loader2 size={18} className="is-spinning" /> {t('projects.docker.loadingLogs')}</div>
              ) : logsError ? (
                <div className="project-docker-error" role="alert"><AlertTriangle size={15} /> {logsError}</div>
              ) : logs?.output ? (
                <pre>{logs.output}</pre>
              ) : (
                <div className="project-docker-logs-state">{t('projects.docker.noLogs')}</div>
              )}
            </div>
            <footer>{t('projects.docker.lastLogLines', 200)}</footer>
          </section>
        </div>
      )}
    </div>
  );
}
