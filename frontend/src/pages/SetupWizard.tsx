import { useState, useEffect } from 'react';
import { setup as setupApi, agents as agentsApi, projects as projectsApi } from '../lib/api';
import type { SetupStatus, AgentDetection, DetectedRepo } from '../types/generated';
import {
  Cpu, FolderSearch, Scan, ChevronRight, Check, Download, Loader2, RefreshCw,
  GitBranch, FolderOpen, Eye,
} from 'lucide-react';

interface Props {
  initialStatus: SetupStatus | null;
  onComplete: () => void;
}

const STEPS = [
  { id: 'agents', label: 'Agents', icon: Cpu },
  { id: 'repos', label: 'Depots', icon: FolderSearch },
  { id: 'done', label: 'Termine', icon: Check },
] as const;

export function SetupWizard({ initialStatus, onComplete }: Props) {
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
      setError(e instanceof Error ? e.message : 'Detection failed');
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
      setError(e instanceof Error ? e.message : 'Install failed');
    } finally {
      setInstalling(null);
    }
  };

  const handleGoToRepos = async () => {
    setStep(1);
    // Auto-scan on entering step 2
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
      setError(e instanceof Error ? e.message : 'Scan failed');
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
      // Create projects from visible (non-hidden) repos
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
    <div style={styles.container}>
      <div style={styles.card}>
        {/* Header */}
        <div style={styles.header}>
          <div style={styles.logo}>&#x26A1;</div>
          <h1 style={styles.title}>Kronn</h1>
          <p style={styles.subtitle}>Enter the grid. Command your agents.</p>
        </div>

        {/* Step indicator */}
        <div style={styles.steps}>
          {STEPS.map((s, i) => {
            const Icon = s.icon;
            const active = i === step;
            const done = i < step;
            return (
              <div key={s.id} style={styles.stepItem(active, done)}>
                <div style={styles.stepIcon(active, done)}>
                  {done ? <Check size={14} /> : <Icon size={14} />}
                </div>
                <span style={styles.stepLabel(active)}>{s.label}</span>
                {i < STEPS.length - 1 && <ChevronRight size={14} style={{ color: 'rgba(255,255,255,0.15)', margin: '0 4px' }} />}
              </div>
            );
          })}
        </div>

        {error && (
          <div style={styles.error}>
            {error}
            <button onClick={() => setError(null)} style={styles.errorClose}>&times;</button>
          </div>
        )}

        <div style={styles.content}>
          {/* ── STEP 0: Agents ── */}
          {step === 0 && (
            <div>
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <h2 style={styles.h2}>Agents IA</h2>
                <button style={styles.btnIcon} onClick={refreshAgents} disabled={detecting} title="Rafraichir">
                  <RefreshCw size={14} style={detecting ? { animation: 'spin 1s linear infinite' } : undefined} />
                </button>
              </div>
              <p style={styles.desc}>
                {installedCount > 0
                  ? `${installedCount} agent${installedCount > 1 ? 's' : ''} detecte${installedCount > 1 ? 's' : ''} sur votre systeme.`
                  : 'Aucun agent detecte — installez-en au moins un.'}
              </p>

              <div style={styles.agentList}>
                {agents.map((agent) => (
                  <div key={agent.name} style={styles.agentRow}>
                    <div style={styles.dot(agent.installed || agent.runtime_available)} />
                    <div style={{ flex: 1 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                        <span style={styles.agentName}>{agent.name}</span>
                        <span style={styles.originBadge}>{agent.origin}</span>
                      </div>
                      {agent.installed ? (
                        <div style={styles.agentMeta}>
                          {agent.version && <code style={styles.code}>v{agent.version}</code>}
                          {agent.latest_version && agent.latest_version !== agent.version && (
                            <span style={styles.updateBadge}>&#x2B06; {agent.latest_version}</span>
                          )}
                        </div>
                      ) : agent.runtime_available ? (
                        <div style={styles.agentMeta}>
                          <span style={{ color: 'rgba(52,211,153,0.7)', fontSize: 11 }}>runtime OK</span>
                          <span style={{ color: 'rgba(255,255,255,0.2)', fontSize: 10 }}> — via npx</span>
                        </div>
                      ) : (
                        <div style={styles.agentMeta}>
                          <code style={styles.code}>{agent.install_command}</code>
                        </div>
                      )}
                    </div>
                    {!agent.installed && !agent.runtime_available && (
                      <button
                        style={styles.btnInstall}
                        onClick={() => handleInstallAgent(agent)}
                        disabled={installing !== null}
                      >
                        {installing === agent.name ? (
                          <><Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> ...</>
                        ) : (
                          <><Download size={14} /> Installer</>
                        )}
                      </button>
                    )}
                    {(agent.installed || agent.runtime_available) && (
                      <span style={styles.badgeOk}><Check size={12} /> OK</span>
                    )}
                  </div>
                ))}
              </div>

              <style>{`@keyframes spin { to { transform: rotate(360deg) } }`}</style>

              <button
                style={{ ...styles.btnPrimary, opacity: installedCount === 0 ? 0.4 : 1, cursor: installedCount === 0 ? 'not-allowed' : 'pointer' }}
                onClick={handleGoToRepos}
                disabled={installedCount === 0}
              >
                Continuer <ChevronRight size={16} />
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
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <h2 style={styles.h2}>Depots detectes</h2>
                <button style={styles.btnIcon} onClick={() => handleScan(paths)} disabled={scanning} title="Re-scanner">
                  <RefreshCw size={14} style={scanning ? { animation: 'spin 1s linear infinite' } : undefined} />
                </button>
              </div>

              {scanning ? (
                <div style={{ textAlign: 'center', padding: '40px 0' }}>
                  <Loader2 size={28} style={{ color: '#c8ff00', animation: 'spin 1s linear infinite' }} />
                  <p style={{ ...styles.desc, marginTop: 12 }}>Scan des depots git...</p>
                </div>
              ) : displayRepos.length > 0 ? (
                <>
                  <p style={styles.desc}>
                    {visibleRepos.length} depot{visibleRepos.length > 1 ? 's' : ''} git trouve{visibleRepos.length > 1 ? 's' : ''}.
                    {hiddenRepos.length > 0 && !showHidden && (
                      <span style={{ color: 'rgba(255,255,255,0.25)' }}> + {hiddenRepos.length} cache{hiddenRepos.length > 1 ? 's' : ''}</span>
                    )}
                  </p>
                  <div style={styles.repoList}>
                    {displayRepos.map((repo) => (
                      <div key={repo.path} style={{ ...styles.repoRow, opacity: repo.hidden ? 0.5 : 1 }}>
                        <GitBranch size={14} style={{ color: 'rgba(255,255,255,0.3)', flexShrink: 0 }} />
                        <div style={{ flex: 1 }}>
                          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                            <span style={styles.repoName}>{repo.name}</span>
                            {repo.hidden && <span style={styles.hiddenBadge}>cache</span>}
                          </div>
                          <div style={styles.repoMeta}>
                            <span>{repo.branch}</span>
                            {repo.ai_configs.length > 0 && (
                              <span style={styles.aiConfigBadge}>
                                {repo.ai_configs.length} config{repo.ai_configs.length > 1 ? 's' : ''} AI
                              </span>
                            )}
                          </div>
                        </div>
                      </div>
                    ))}
                  </div>
                  <div style={{ display: 'flex', gap: 12, marginTop: 8 }}>
                    {hiddenRepos.length > 0 && (
                      <button style={styles.btnText} onClick={() => setShowHidden(!showHidden)}>
                        <Eye size={12} /> {showHidden ? 'Masquer' : 'Voir'} les {hiddenRepos.length} depot{hiddenRepos.length > 1 ? 's' : ''} cache{hiddenRepos.length > 1 ? 's' : ''}
                      </button>
                    )}
                    {!showManualPath && (
                      <button style={styles.btnText} onClick={() => setShowManualPath(true)}>
                        <FolderOpen size={12} /> Ajouter un autre chemin
                      </button>
                    )}
                  </div>
                </>
              ) : (
                <div style={{ textAlign: 'center', padding: '24px 0' }}>
                  <FolderSearch size={32} style={{ color: 'rgba(255,255,255,0.15)', marginBottom: 12 }} />
                  <p style={styles.desc}>Aucun depot git trouve. Indiquez un chemin a scanner.</p>
                </div>
              )}

              {showManualPath && (
                <div style={{ marginTop: 16 }}>
                  <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
                    <input
                      style={{ ...styles.input, flex: 1 }}
                      placeholder="~/work, ~/projects, ..."
                      value={newPath}
                      onChange={(e) => setNewPath(e.target.value)}
                      onKeyDown={(e) => { if (e.key === 'Enter') handleAddPath(); }}
                    />
                    <button style={styles.btnSecondary} onClick={handleAddPath}>
                      <Scan size={14} /> Scanner
                    </button>
                  </div>
                  {paths.length > 0 && (
                    <div style={styles.pathList}>
                      {paths.map((p, i) => (
                        <div key={i} style={styles.pathRow}>
                          <code style={styles.code}>{p}</code>
                          <button style={styles.btnRemove} onClick={() => setPaths(paths.filter((_, j) => j !== i))}>&times;</button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}

              <button
                style={{ ...styles.btnPrimary, opacity: visibleRepos.length === 0 ? 0.4 : 1, cursor: visibleRepos.length === 0 ? 'not-allowed' : 'pointer' }}
                onClick={() => setStep(2)}
                disabled={visibleRepos.length === 0}
              >
                Continuer <ChevronRight size={16} />
              </button>
            </div>
            );
          })()}

          {/* ── STEP 2: Done ── */}
          {step === 2 && (
            <div style={{ textAlign: 'center', padding: '24px 0' }}>
              <div style={{ fontSize: 48, marginBottom: 12 }}>&#x2713;</div>
              <h2 style={styles.h2}>Configuration terminee</h2>
              <p style={styles.desc}>
                {installedCount} agent{installedCount > 1 ? 's' : ''}, {repos.length} depot{repos.length > 1 ? 's' : ''} detecte{repos.length > 1 ? 's' : ''}.
              </p>
              <button style={styles.btnPrimary} onClick={handleComplete}>
                Acceder au dashboard <ChevronRight size={16} />
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ─── Styles ─────────────────────────────────────────────────────────────────

const styles = {
  container: { display: 'flex', alignItems: 'center', justifyContent: 'center', minHeight: '100vh', padding: 24 } as const,
  card: { background: '#12151c', border: '1px solid rgba(255,255,255,0.07)', borderRadius: 16, padding: 32, width: '100%', maxWidth: 560 } as const,
  header: { textAlign: 'center' as const, marginBottom: 28 },
  logo: { fontSize: 32, marginBottom: 8 },
  title: { fontSize: 24, fontWeight: 700, letterSpacing: '-0.03em' } as const,
  subtitle: { color: 'rgba(255,255,255,0.4)', fontSize: 13, marginTop: 4 } as const,
  steps: { display: 'flex', alignItems: 'center', justifyContent: 'center', marginBottom: 28, gap: 4 } as const,
  stepItem: (active: boolean, done: boolean) => ({ display: 'flex', alignItems: 'center', gap: 6, opacity: active || done ? 1 : 0.35 } as const),
  stepIcon: (active: boolean, done: boolean) => ({ width: 28, height: 28, borderRadius: '50%', display: 'flex', alignItems: 'center', justifyContent: 'center', background: done ? 'rgba(52,211,153,0.15)' : active ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.05)', color: done ? '#34d399' : active ? '#c8ff00' : 'rgba(255,255,255,0.4)' } as const),
  stepLabel: (active: boolean) => ({ fontSize: 12, fontWeight: active ? 600 : 400, color: active ? '#e8eaed' : 'rgba(255,255,255,0.4)' } as const),
  content: { minHeight: 300 } as const,
  h2: { fontSize: 17, fontWeight: 600, marginBottom: 8, letterSpacing: '-0.01em' } as const,
  desc: { color: 'rgba(255,255,255,0.45)', fontSize: 13, lineHeight: 1.6, marginBottom: 20 } as const,
  input: { width: '100%', padding: '10px 14px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'JetBrains Mono, monospace', outline: 'none' } as const,
  btnPrimary: { width: '100%', padding: '12px 20px', background: '#c8ff00', color: '#0a0c10', border: 'none', borderRadius: 8, fontSize: 14, fontWeight: 700, cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8, marginTop: 20, fontFamily: 'inherit' } as const,
  btnSecondary: { padding: '10px 18px', background: 'rgba(255,255,255,0.06)', color: '#e8eaed', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 8, fontSize: 13, cursor: 'pointer', fontFamily: 'inherit', display: 'flex', alignItems: 'center', gap: 6 } as const,
  btnIcon: { background: 'none', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 6, padding: '6px 8px', cursor: 'pointer', color: 'rgba(255,255,255,0.5)', display: 'flex', alignItems: 'center' } as const,
  btnText: { background: 'none', border: 'none', color: 'rgba(255,255,255,0.4)', cursor: 'pointer', fontSize: 12, display: 'flex', alignItems: 'center', gap: 6, padding: '4px 0', fontFamily: 'inherit' } as const,
  btnInstall: { padding: '6px 14px', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', border: '1px solid rgba(200,255,0,0.2)', borderRadius: 6, fontSize: 12, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'inherit' } as const,
  btnRemove: { background: 'none', border: 'none', color: 'rgba(255,255,255,0.3)', cursor: 'pointer', fontSize: 16, padding: '4px 8px' } as const,
  error: { background: 'rgba(255,77,106,0.1)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 8, padding: '10px 14px', marginBottom: 16, fontSize: 12, color: '#ff4d6a', display: 'flex', justifyContent: 'space-between', alignItems: 'center' } as const,
  errorClose: { background: 'none', border: 'none', color: '#ff4d6a', cursor: 'pointer', fontSize: 16 } as const,
  agentList: { display: 'flex', flexDirection: 'column' as const, gap: 8, marginBottom: 8 },
  agentRow: { display: 'flex', alignItems: 'center', gap: 12, padding: '12px 14px', borderRadius: 8, background: 'rgba(255,255,255,0.03)' } as const,
  agentName: { fontWeight: 600, fontSize: 13 } as const,
  agentMeta: { display: 'flex', gap: 8, alignItems: 'center', fontSize: 11, marginTop: 2, color: 'rgba(255,255,255,0.35)' } as const,
  dot: (on: boolean) => ({ width: 8, height: 8, borderRadius: '50%', background: on ? '#34d399' : 'rgba(255,255,255,0.15)', boxShadow: on ? '0 0 8px rgba(52,211,153,0.5)' : 'none', flexShrink: 0 } as const),
  badgeOk: { display: 'flex', alignItems: 'center', gap: 4, fontSize: 11, color: '#34d399', padding: '4px 10px', borderRadius: 20, background: 'rgba(52,211,153,0.1)' } as const,
  originBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(100,180,255,0.1)', color: 'rgba(100,180,255,0.7)', border: '1px solid rgba(100,180,255,0.15)' } as const,
  updateBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(255,200,0,0.1)', color: '#ffc800' } as const,
  code: { fontSize: 11, fontFamily: 'JetBrains Mono, monospace', background: 'rgba(255,255,255,0.06)', padding: '2px 6px', borderRadius: 4 } as const,
  repoList: { display: 'flex', flexDirection: 'column' as const, gap: 6, maxHeight: 320, overflowY: 'auto' as const },
  repoRow: { display: 'flex', alignItems: 'center', gap: 10, padding: '10px 12px', borderRadius: 8, background: 'rgba(255,255,255,0.03)' } as const,
  repoName: { fontWeight: 600, fontSize: 13 } as const,
  repoMeta: { display: 'flex', gap: 8, alignItems: 'center', fontSize: 11, marginTop: 2, color: 'rgba(255,255,255,0.35)' } as const,
  aiConfigBadge: { fontSize: 10, padding: '1px 6px', borderRadius: 4, background: 'rgba(52,211,153,0.1)', color: '#34d399' } as const,
  hiddenBadge: { fontSize: 9, padding: '1px 5px', borderRadius: 3, background: 'rgba(255,255,255,0.06)', color: 'rgba(255,255,255,0.3)' } as const,
  pathList: { display: 'flex', flexDirection: 'column' as const, gap: 6 },
  pathRow: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '8px 12px', borderRadius: 6, background: 'rgba(255,255,255,0.03)' } as const,
};
