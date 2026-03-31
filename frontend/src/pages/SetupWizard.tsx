import { useState, useEffect } from 'react';
import { setup as setupApi, agents as agentsApi, projects as projectsApi } from '../lib/api';
import type { SetupStatus, AgentDetection, DetectedRepo } from '../types/generated';
import { useT } from '../lib/I18nContext';
import {
  Cpu, FolderSearch, Scan, ChevronRight, Check, Download, Loader2, RefreshCw,
  GitBranch, FolderOpen, Eye,
} from 'lucide-react';
import './SetupWizard.css';

interface Props {
  initialStatus: SetupStatus | null;
  onComplete: () => void;
}

export function SetupWizard({ initialStatus, onComplete }: Props) {
  const { t } = useT();

  const STEPS = [
    { id: 'agents', label: t('setup.step.agents'), icon: Cpu },
    { id: 'repos', label: t('setup.step.repos'), icon: FolderSearch },
    { id: 'done', label: t('setup.step.done'), icon: Check },
  ] as const;
  const [step, setStep] = useState(0);
  const [agents, setAgents] = useState<AgentDetection[]>(initialStatus?.agents_detected ?? []);
  const [repos, setRepos] = useState<DetectedRepo[]>(initialStatus?.repos_detected ?? []);
  const [scanning, setScanning] = useState(false);
  const [installing, setInstalling] = useState<string | null>(null);
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showManualPath, setShowManualPath] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const [paths, setPaths] = useState<string[]>([]);
  const [newPath, setNewPath] = useState('');

  const installedCount = agents.filter(a => a.installed || a.runtime_available).length;

  useEffect(() => {
    refreshAgents();
  }, []);

  const refreshAgents = async () => {
    setDetecting(true);
    try {
      const detected = await agentsApi.detect();
      setAgents(detected);
    } catch (e) {
      setError(e instanceof Error ? e.message : t('setup.detectionFailed'));
    } finally {
      setDetecting(false);
    }
  };

  const handleInstallAgent = async (agent: AgentDetection) => {
    setInstalling(agent.name);
    setError(null);
    try {
      await agentsApi.install(agent.agent_type);
      await refreshAgents();
    } catch (e) {
      setError(e instanceof Error ? e.message : t('setup.installFailed'));
    } finally {
      setInstalling(null);
    }
  };

  const handleGoToRepos = async () => {
    setStep(1);
    if (repos.length === 0) {
      await handleScan();
    }
  };

  const handleScan = async (extraPaths?: string[]) => {
    setScanning(true);
    setError(null);
    try {
      if (extraPaths && extraPaths.length > 0) {
        await setupApi.setScanPaths({ paths: extraPaths });
      }
      const status = await setupApi.getStatus();
      setRepos(status.repos_detected);
      if (status.repos_detected.length === 0 && !showManualPath) {
        setShowManualPath(true);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : t('setup.scanFailed'));
    } finally {
      setScanning(false);
    }
  };

  const handleAddPath = () => {
    if (newPath) {
      const updated = [...paths, newPath];
      setPaths(updated);
      setNewPath('');
      handleScan(updated);
    }
  };

  const handleComplete = async () => {
    try {
      await setupApi.complete();
      for (const repo of repos.filter(r => !r.hidden)) {
        try {
          await projectsApi.create(repo);
        } catch {
          // Skip repos that fail
        }
      }
    } catch {
      // Continue even if save fails
    }
    onComplete();
  };

  return (
    <div className="setup-container">
      <div className="setup-card">
        {/* Header */}
        <div className="setup-header">
          <div className="setup-logo">&#x26A1;</div>
          <h1 className="setup-title">Kronn</h1>
          <p className="setup-subtitle">{t('setup.subtitle')}</p>
        </div>

        {/* Step indicator */}
        <div className="setup-steps">
          {STEPS.map((s, i) => {
            const Icon = s.icon;
            const active = i === step;
            const done = i < step;
            return (
              <div key={s.id} className="setup-step-item" data-active={active} data-done={done}>
                <div className="setup-step-icon">
                  {done ? <Check size={14} /> : <Icon size={14} />}
                </div>
                <span className="setup-step-label">{s.label}</span>
                {i < STEPS.length - 1 && <ChevronRight size={14} className="setup-step-separator" />}
              </div>
            );
          })}
        </div>

        {error && (
          <div className="setup-error">
            {error}
            <button onClick={() => setError(null)} className="setup-error-close">&times;</button>
          </div>
        )}

        <div className="setup-content">
          {/* ── STEP 0: Agents ── */}
          {step === 0 && (
            <div>
              <div className="flex-between">
                <h2 className="setup-h2">{t('setup.aiAgents')}</h2>
                <button className="btn btn-icon btn-secondary" onClick={refreshAgents} disabled={detecting} title={t('setup.refresh')}>
                  <RefreshCw size={14} style={detecting ? { animation: 'spin 1s linear infinite' } : undefined} />
                </button>
              </div>
              <p className="setup-desc">
                {installedCount > 0
                  ? t('setup.agentsDetected', installedCount, installedCount > 1 ? 's' : '', installedCount > 1 ? 's' : '')
                  : t('setup.noAgentDetected')}
              </p>

              <div className="setup-agent-list">
                {agents.map((agent) => (
                  <div key={agent.name} className="setup-agent-row">
                    <div className={`dot ${agent.installed ? 'dot-on' : agent.runtime_available ? 'dot-warn' : 'dot-off'}`} />
                    <div className="flex-1">
                      <div className="flex-row gap-4">
                        <span className="setup-agent-name">{agent.name}</span>
                        <span className="setup-badge-origin">{agent.origin}</span>
                      </div>
                      {agent.installed ? (
                        <div className="setup-agent-meta">
                          {agent.version && <code className="code">v{agent.version}</code>}
                          {agent.latest_version && agent.latest_version !== agent.version && (
                            <span className="setup-badge-update">&#x2B06; {agent.latest_version}</span>
                          )}
                        </div>
                      ) : agent.runtime_available ? (
                        <div className="setup-agent-meta">
                          <span style={{ color: 'rgba(52,211,153,0.7)' }}>runtime OK</span>
                          <span className="text-ghost text-xs"> — via npx</span>
                        </div>
                      ) : (
                        <div className="setup-agent-meta">
                          <code className="code">{agent.install_command}</code>
                        </div>
                      )}
                    </div>
                    {!agent.installed && !agent.runtime_available && (
                      <button
                        className="btn btn-accent btn-sm"
                        onClick={() => handleInstallAgent(agent)}
                        disabled={installing !== null}
                      >
                        {installing === agent.name ? (
                          <><Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> ...</>
                        ) : (
                          <><Download size={14} /> {t('setup.install')}</>
                        )}
                      </button>
                    )}
                    {agent.installed && (
                      <span className="setup-badge-ok"><Check size={12} /> OK</span>
                    )}
                    {!agent.installed && agent.runtime_available && (
                      <span className="setup-badge-ok" style={{ background: 'rgba(255,165,0,0.15)', color: '#ffa500', borderColor: 'rgba(255,165,0,0.3)' }}>npx</span>
                    )}
                  </div>
                ))}
              </div>

              <button
                className="setup-btn-primary"
                onClick={handleGoToRepos}
              >
                {installedCount > 0
                  ? <>{t('setup.continue')} <ChevronRight size={16} /></>
                  : <>{t('setup.skip')} <ChevronRight size={16} /></>}
              </button>
            </div>
          )}

          {/* ── STEP 1: Repos ── */}
          {step === 1 && (() => {
            const visibleRepos = repos.filter(r => !r.hidden);
            const hiddenRepos = repos.filter(r => r.hidden);
            const displayRepos = showHidden ? repos : visibleRepos;
            return (
            <div>
              <div className="flex-between">
                <h2 className="setup-h2">{t('setup.detectedRepos')}</h2>
                <button className="btn btn-icon btn-secondary" onClick={() => handleScan(paths)} disabled={scanning} title={t('setup.rescan')}>
                  <RefreshCw size={14} style={scanning ? { animation: 'spin 1s linear infinite' } : undefined} />
                </button>
              </div>

              {scanning ? (
                <div className="text-center py-8">
                  <Loader2 size={28} className="text-accent" style={{ animation: 'spin 1s linear infinite' }} />
                  <p className="setup-desc mt-6">{t('setup.scanningRepos')}</p>
                </div>
              ) : displayRepos.length > 0 ? (
                <>
                  <p className="setup-desc">
                    {t('setup.reposFound', visibleRepos.length, visibleRepos.length > 1 ? 's' : '', visibleRepos.length > 1 ? 's' : '')}
                    {hiddenRepos.length > 0 && !showHidden && (
                      <span className="text-faint"> {t('setup.hiddenRepos', hiddenRepos.length, hiddenRepos.length > 1 ? 's' : '')}</span>
                    )}
                  </p>
                  <div className="setup-repo-list">
                    {displayRepos.map((repo) => (
                      <div key={repo.path} className="setup-repo-row" style={repo.hidden ? { opacity: 0.5 } : undefined}>
                        <GitBranch size={14} className="text-dim flex-shrink-0" />
                        <div className="flex-1">
                          <div className="flex-row gap-3">
                            <span className="setup-repo-name">{repo.name}</span>
                            {repo.hidden && <span className="setup-badge-hidden">{t('setup.hidden')}</span>}
                          </div>
                          <div className="setup-repo-meta">
                            <span>{repo.branch}</span>
                            {repo.ai_configs.length > 0 && (
                              <span className="setup-badge-ai-config">
                                {repo.ai_configs.length} config{repo.ai_configs.length > 1 ? 's' : ''} AI
                              </span>
                            )}
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                  <div className="flex-row gap-6 mt-4">
                    {hiddenRepos.length > 0 && (
                      <button className="btn btn-ghost btn-sm" onClick={() => setShowHidden(!showHidden)}>
                        <Eye size={12} /> {t('setup.hiddenReposToggle', showHidden ? t('setup.hide') : t('setup.show'), hiddenRepos.length, hiddenRepos.length > 1 ? 's' : '', hiddenRepos.length > 1 ? 's' : '')}
                      </button>
                    )}
                    {!showManualPath && (
                      <button className="btn btn-ghost btn-sm" onClick={() => setShowManualPath(true)}>
                        <FolderOpen size={12} /> {t('setup.addPath')}
                      </button>
                    )}
                  </div>
                </>
              ) : (
                <div className="text-center py-8">
                  <FolderSearch size={32} className="text-ghost mb-6" />
                  <p className="setup-desc">{t('setup.noRepoFound')}</p>
                </div>
              )}

              {showManualPath && (
                <div className="mt-8">
                  <div className="flex-row gap-4 mb-4">
                    <input
                      className="input mono flex-1"
                      placeholder="~/work, ~/projects, ..."
                      value={newPath}
                      onChange={(e) => setNewPath(e.target.value)}
                      onKeyDown={(e) => { if (e.key === 'Enter') handleAddPath(); }}
                    />
                    <button className="btn btn-secondary" onClick={handleAddPath}>
                      <Scan size={14} /> {t('setup.scan')}
                    </button>
                  </div>
                  {paths.length > 0 && (
                    <div className="setup-path-list">
                      {paths.map((p, i) => (
                        <div key={i} className="setup-path-row">
                          <code className="code">{p}</code>
                          <button className="btn btn-ghost btn-icon" onClick={() => setPaths(paths.filter((_, j) => j !== i))}>&times;</button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              <button
                className="setup-btn-primary"
                onClick={() => setStep(2)}
              >
                {visibleRepos.length > 0
                  ? <>{t('setup.continue')} <ChevronRight size={16} /></>
                  : <>{t('setup.skip')} <ChevronRight size={16} /></>}
              </button>
            </div>
            );
          })()}

          {/* ── STEP 2: Done ── */}
          {step === 2 && (
            <div className="text-center py-8">
              <div className="setup-done-icon">&#x2713;</div>
              <h2 className="setup-h2">{t('setup.configDone')}</h2>
              <p className="setup-desc">
                {t('setup.summary', installedCount, installedCount > 1 ? 's' : '', repos.length, repos.length > 1 ? 's' : '', repos.length > 1 ? 's' : '')}
              </p>
              <button className="setup-btn-primary" onClick={handleComplete}>
                {t('setup.goToDashboard')} <ChevronRight size={16} />
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
