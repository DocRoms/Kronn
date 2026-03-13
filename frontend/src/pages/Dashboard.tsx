import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { projects as projectsApi, mcps as mcpsApi, agents as agentsApi, discussions as discussionsApi, workflows as workflowsApi, config as configApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useToast } from '../hooks/useToast';
import type { Project, AgentDetection, AgentType } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { McpPage } from './McpPage';
import { WorkflowsPage } from './WorkflowsPage';
import { SettingsPage } from './SettingsPage';
import { DiscussionsPage } from './DiscussionsPage';
import {
  Folder, Server, ChevronRight, Cpu, Workflow,
  Plus, Trash2, Search, Zap, Settings, Eye,
  Download, Loader2,
  MessageSquare, X, AlertTriangle,
  Play, FileCode, ShieldCheck,
} from 'lucide-react';

type Page = 'projects' | 'mcps' | 'workflows' | 'discussions' | 'settings';

interface DashboardProps {
  onReset: () => void;
}

const isHiddenPath = (path: string) => path.split('/').some(s => s.startsWith('.'));

const isAiReady = (p: Project) => p.audit_status !== 'NoTemplate';

/** Agent is usable: locally installed OR available via npx/uvx runtime fallback */
const isUsable = (a: AgentDetection) => (a.installed || a.runtime_available) && a.enabled;

const isValidationDisc = (title: string) => title === 'Validation audit AI';

const AI_CONFIG_LABELS: Record<string, string> = {
  ClaudeMd: 'CLAUDE.md',
  ClauseDir: '.claude/',
  AiDir: '.ai/',
  CursorRules: '.cursorrules',
  ContinueDev: '.continue/',
  McpJson: '.mcp.json',
  Custom: 'custom',
};

// Sort score for project readiness
const readinessScore = (p: Project) => {
  return p.ai_config.detected ? 0 : 1;
};

export function Dashboard({ onReset }: DashboardProps) {
  const { t } = useT();
  const { toast, ToastContainer } = useToast();
  const [page, setPage] = useState<Page>('projects');
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  // Cross-page prefill for discussion creation (e.g. "validate audit" from Projects)
  const [discPrefill, setDiscPrefill] = useState<{ projectId: string; title: string; prompt: string } | null>(null);
  // Unseen message tracking (persisted in localStorage, computed in Dashboard)
  const [lastSeenMsgCount, setLastSeenMsgCount] = useState<Record<string, number>>(() => {
    try { return JSON.parse(localStorage.getItem('kronn:lastSeenMsgCount') ?? '{}'); } catch { return {}; }
  });
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(null);
  // AI audit state
  const [installingTemplate, setInstallingTemplate] = useState<string | null>(null);
  const [auditAgentChoice, setAuditAgentChoice] = useState<Record<string, AgentType>>({});
  const [auditState, setAuditState] = useState<Record<string, {
    active: boolean;
    step: number;
    totalSteps: number;
    currentFile: string;
  }>>({});

  // ─── Lifted discussion streaming state (survives page changes) ──────────
  const [sendingMap, setSendingMap] = useState<Record<string, boolean>>({});
  const [streamingMap, setStreamingMap] = useState<Record<string, string>>({});
  const abortControllers = useRef<Record<string, AbortController>>({});

  const cleanupStream = useCallback((discId: string) => {
    setSendingMap(prev => ({ ...prev, [discId]: false }));
    setStreamingMap(prev => { const n = { ...prev }; delete n[discId]; return n; });
    delete abortControllers.current[discId];
  }, []);

  const { data: projectList, refetch } = useApi(() => projectsApi.list(), []);
  const { data: registry } = useApi(() => mcpsApi.registry(), []);
  const { data: mcpOverviewData, refetch: refetchMcps } = useApi(() => mcpsApi.overview(), []);
  const { data: agentList, refetch: refetchAgents } = useApi(() => agentsApi.detect(), []);
  const { data: discussionList, refetch: refetchDiscussions } = useApi(() => discussionsApi.list(), []);
  const { data: configLanguage, refetch: refetchLanguage } = useApi(() => configApi.getLanguage(), []);
  const { data: agentAccess, refetch: refetchAgentAccess } = useApi(() => configApi.getAgentAccess(), []);
  const { data: workflowList, refetch: refetchWorkflows } = useApi(() => workflowsApi.list(), []);

  // Poll discussions for notifications — faster when on discussions page, slower otherwise
  useEffect(() => {
    const pollInterval = page === 'discussions' ? 5000 : 30000;
    const interval = setInterval(() => { refetchDiscussions(); }, pollInterval);
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') refetchDiscussions();
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => { clearInterval(interval); document.removeEventListener('visibilitychange', onVisibilityChange); };
  }, [refetchDiscussions, page]);

  // Poll workflows — fast when running, slow otherwise
  const runningWorkflows = useMemo(() =>
    (workflowList ?? []).filter(w => w.last_run?.status === 'Running' || w.last_run?.status === 'Pending').length,
    [workflowList],
  );

  useEffect(() => {
    const pollInterval = runningWorkflows > 0 ? 3000 : page === 'workflows' ? 10000 : 60000;
    const interval = setInterval(() => { refetchWorkflows(); }, pollInterval);
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') refetchWorkflows();
    };
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => { clearInterval(interval); document.removeEventListener('visibilitychange', onVisibilityChange); };
  }, [refetchWorkflows, page, runningWorkflows]);

  const projects = projectList ?? [];
  const mcpRegistry = registry ?? [];
  const mcpOverview = mcpOverviewData ?? { servers: [], configs: [], customized_contexts: [] };
  const agents = agentList ?? [];
  const allDiscussions = discussionList ?? [];

  // ─── Unseen count (computed here so it works across all pages) ─────────
  const markDiscussionSeen = useCallback((discId: string, msgCount: number) => {
    setLastSeenMsgCount(prev => {
      const next = { ...prev, [discId]: msgCount };
      localStorage.setItem('kronn:lastSeenMsgCount', JSON.stringify(next));
      return next;
    });
  }, []);

  const totalUnseen = useMemo(() => allDiscussions.reduce((acc, disc) => {
    const unseen = (disc.message_count ?? disc.messages.length) - (lastSeenMsgCount[disc.id] ?? 0);
    return acc + (unseen > 0 && disc.id !== activeDiscussionId ? unseen : 0);
  }, 0), [allDiscussions, lastSeenMsgCount, activeDiscussionId]);

  useEffect(() => {
    document.title = totalUnseen > 0 ? `(${totalUnseen}) Kronn` : 'Kronn';
  }, [totalUnseen]);

  // Stable callback for prefill consumed
  const handlePrefillConsumed = useCallback(() => setDiscPrefill(null), []);

  const handleDeleteProject = async (id: string) => {
    await projectsApi.delete(id);
    refetch();
  };

  const handleInstallTemplate = async (projectId: string) => {
    setInstallingTemplate(projectId);
    try {
      await projectsApi.installTemplate(projectId);
      refetch();
    } catch (e) {
      console.error('Failed to install template:', e);
    } finally {
      setInstallingTemplate(null);
    }
  };

  const handleLaunchAudit = async (projectId: string) => {
    setAuditState(prev => ({
      ...prev,
      [projectId]: { active: true, step: 0, totalSteps: 10, currentFile: 'Demarrage...' }
    }));
    try {
      const auditAgent = auditAgentChoice[projectId] ?? agents.filter(isUsable)[0]?.agent_type ?? 'ClaudeCode';
      await projectsApi.auditStream(projectId, { agent: auditAgent }, {
        onStepStart: (step, total, file) => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { active: true, step, totalSteps: total, currentFile: file }
          }));
        },
        onChunk: () => { /* progress tracked via step_start */ },
        onStepDone: () => { /* step progress */ },
        onDone: () => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { ...prev[projectId], active: false }
          }));
          refetch();
        },
        onError: (error) => {
          console.error('Audit error:', error);
          setAuditState(prev => ({
            ...prev,
            [projectId]: { ...prev[projectId], active: false }
          }));
        },
      });
    } catch (e) {
      console.error('Audit failed:', e);
      setAuditState(prev => ({
        ...prev,
        [projectId]: { ...prev[projectId], active: false }
      }));
    }
  };

  const handleScan = async () => {
    const repos = await projectsApi.scan();
    for (const repo of repos) {
      if (!repo.has_project && !repo.hidden) {
        await projectsApi.create(repo);
      }
    }
    refetch();
  };

  return (
    <div style={s.app}>
      <ToastContainer />
      <style>{`@keyframes spin { to { transform: rotate(360deg) } }`}</style>
      {/* Nav */}
      <nav style={s.nav}>
        <div style={s.navBrand}>
          <Zap size={18} style={{ color: '#c8ff00' }} />
          <span style={s.navTitle}>Kronn</span>
        </div>
        {([
          ['projects', Folder, t('nav.projects')],
          ['discussions', MessageSquare, t('nav.discussions')],
          ['mcps', Server, t('nav.mcps')],
          ['workflows', Workflow, t('nav.workflows')],
          ['settings', Settings, t('nav.config')],
        ] as [string, typeof Folder, string][]).map(([id, Icon, label]) => (
          <button key={id} style={{ ...s.navBtn(page === id), position: 'relative' }} onClick={() => setPage(id as Page)}>
            {id === 'workflows' && runningWorkflows > 0
              ? <Loader2 size={14} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />
              : <Icon size={14} />
            }
            {' '}{label}
            {id === 'discussions' && totalUnseen > 0 && (
              <span style={{
                position: 'absolute', top: 2, right: 2,
                background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center',
                lineHeight: '12px',
              }}>{totalUnseen}</span>
            )}
            {id === 'workflows' && runningWorkflows > 0 && (
              <span style={{
                position: 'absolute', top: 2, right: 2,
                background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center',
                lineHeight: '12px',
              }}>{runningWorkflows}</span>
            )}
          </button>
        ))}
        <div style={{ flex: 1 }} />
        <button style={s.scanBtn} onClick={handleScan}>
          <Search size={14} /> {t('nav.scan')}
        </button>
      </nav>

      {/* Content */}
      <main style={s.main}>

        {/* ════════ PROJETS ════════ */}
        {page === 'projects' && (() => {
          const visibleProjects = projects.filter(p => !isHiddenPath(p.path));
          const hiddenProjects = projects.filter(p => isHiddenPath(p.path));
          const baseProjects = showHidden ? projects : visibleProjects;
          // Filter by search
          const searchLower = projectSearch.toLowerCase();
          const filteredProjects = projectSearch
            ? baseProjects.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower))
            : baseProjects;
          // Sort: AI+MCP > AI only > MCP only > nothing, then by name
          const sortedProjects = [...filteredProjects].sort((a, b) => {
            const diff = readinessScore(a) - readinessScore(b);
            return diff !== 0 ? diff : a.name.localeCompare(b.name);
          });
          const displayProjects = projectSearch ? sortedProjects : sortedProjects.slice(0, projectDisplayLimit);
          const remainingCount = sortedProjects.length - displayProjects.length;
          const aiCount = visibleProjects.filter(isAiReady).length;
          return (
          <div>
            <div style={s.pageHeader}>
              <div>
                <h1 style={s.h1}>{t('projects.title')}</h1>
                <p style={s.meta}>
                  {aiCount}/{visibleProjects.length} {t('projects.aiReady')}
                  {hiddenProjects.length > 0 && (
                    <span style={{ color: 'rgba(255,255,255,0.25)' }}> + {hiddenProjects.length} {hiddenProjects.length > 1 ? t('projects.hiddenPlural') : t('projects.hidden')}</span>
                  )}
                </p>
              </div>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                {hiddenProjects.length > 0 && (
                  <button style={s.iconBtn} onClick={() => setShowHidden(!showHidden)} title={showHidden ? t('projects.hideHidden') : t('projects.showHidden')}>
                    <Eye size={14} style={{ color: showHidden ? '#c8ff00' : undefined }} />
                  </button>
                )}
              </div>
            </div>

            {/* Search bar for projects */}
            {baseProjects.length > 3 && (
              <div style={{ position: 'relative', marginBottom: 12 }}>
                <Search size={14} style={{ position: 'absolute', left: 10, top: '50%', transform: 'translateY(-50%)', color: 'rgba(255,255,255,0.25)', pointerEvents: 'none' }} />
                <input
                  style={{ background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, padding: '8px 12px 8px 32px', color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', width: '100%', outline: 'none' }}
                  placeholder={t('projects.search')}
                  value={projectSearch}
                  onChange={(e) => setProjectSearch(e.target.value)}
                />
                {projectSearch && (
                  <button
                    style={{ position: 'absolute', right: 8, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'rgba(255,255,255,0.3)', padding: 2 }}
                    onClick={() => setProjectSearch('')}
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
            )}

            {displayProjects.map((proj: Project) => {
              const isOpen = expandedId === proj.id;
              const projHidden = isHiddenPath(proj.path);
              const validationDisc = allDiscussions.find(d => d.project_id === proj.id && d.title === 'Validation audit AI');
              const validationInProgress = !!validationDisc && proj.audit_status === 'Audited';
              return (
                <div key={proj.id} style={{ ...s.card(isOpen), opacity: projHidden ? 0.5 : 1 }}>
                  <div style={s.cardHeader} onClick={() => setExpandedId(isOpen ? null : proj.id)}>
                    <ChevronRight size={14} style={{ color: '#c8ff00', transform: isOpen ? 'rotate(90deg)' : 'none', transition: 'transform 0.2s' }} />
                    <div style={{ flex: 1 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                        <span style={s.projName}>{proj.name}</span>
                        {/* AI context badge */}
                        {proj.audit_status === 'NoTemplate' ? (
                          <span style={s.badgeGray}><FileCode size={9} /> AI context</span>
                        ) : (
                          <span style={s.badgeGreen}><FileCode size={9} /> AI context</span>
                        )}
                        {/* AI audit badge */}
                        {auditState[proj.id]?.active ? (
                          <span style={s.badgeOrange}>
                            <Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> AI audit {auditState[proj.id].step}/{auditState[proj.id].totalSteps}
                          </span>
                        ) : (proj.audit_status === 'Audited' || proj.audit_status === 'Validated') ? (
                          <span style={s.badgeGreen}><Cpu size={9} /> AI audit</span>
                        ) : proj.audit_status === 'TemplateInstalled' ? (
                          <span style={s.badgeOrange}><Cpu size={9} /> AI audit</span>
                        ) : (
                          <span style={s.badgeGray}><Cpu size={9} /> AI audit</span>
                        )}
                        {/* Validated badge */}
                        {proj.audit_status === 'Validated' ? (
                          <span style={s.badgeGreen}><ShieldCheck size={9} /> Validated</span>
                        ) : validationInProgress ? (
                          <span style={{ ...s.badgeOrange, cursor: 'pointer' }} onClick={(e) => { e.stopPropagation(); setPage('discussions'); }}>
                            <Loader2 size={9} style={{ animation: 'spin 1s linear infinite' }} /> Validation
                          </span>
                        ) : (proj.audit_status === 'Audited' || proj.audit_status === 'TemplateInstalled') ? (
                          <span style={s.badgeGray}><ShieldCheck size={9} /> Validated</span>
                        ) : null}
                      </div>
                      {/* AI config files on second line */}
                      {proj.ai_config.configs.length > 0 && (
                        <div style={{ display: 'flex', alignItems: 'center', gap: 4, marginTop: 4, flexWrap: 'wrap' }}>
                          {proj.ai_config.configs.map(c => (
                            <span key={c} style={s.aiBadge}>{AI_CONFIG_LABELS[c] ?? c}</span>
                          ))}
                        </div>
                      )}
                      <div style={s.projPath}>{proj.path}</div>
                    </div>
                    <div style={s.projMeta}>
                      <span style={s.metaItem}><Server size={12} /> {mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id)).length}</span>
                      <span style={s.metaItem}><MessageSquare size={12} /> {allDiscussions.filter(d => d.project_id === proj.id).length}</span>
                    </div>
                  </div>

                  {isOpen && (
                    <div style={s.cardBody} onClick={(e) => e.stopPropagation()}>
                      {/* MCPs */}
                      {(() => {
                        const projMcps = mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id));
                        return (
                          <div style={s.section}>
                            <div style={s.sectionHeader}>
                              <Server size={14} /> <span style={s.sectionTitle}>MCP</span>
                              <span style={s.count}>{projMcps.length}</span>
                            </div>
                            {projMcps.map(cfg => (
                              <div key={cfg.id} style={s.row}>
                                <div style={s.dot(true)} />
                                <div style={{ flex: 1 }}>
                                  <span style={{ fontWeight: 600, fontSize: 12 }}>{cfg.server_name}</span>
                                  <span style={{ marginLeft: 6, fontSize: 11, color: 'rgba(255,255,255,0.3)' }}>{cfg.label}</span>
                                  {cfg.is_global && <span style={{ marginLeft: 4, fontSize: 9, color: '#c8ff00' }}>GLOBAL</span>}
                                </div>
                                <button
                                  style={s.iconBtn}
                                  onClick={() => setPage('mcps')}
                                  title={t('projects.manageMcps')}
                                >
                                  <ChevronRight size={12} />
                                </button>
                              </div>
                            ))}
                            {projMcps.length === 0 && (
                              <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.25)', padding: '4px 0' }}>
                                {t('projects.noMcp').split(' — ')[0]} — <button style={{ ...s.iconBtn, fontSize: 11, color: '#c8ff00' }} onClick={() => setPage('mcps')}>{t('projects.noMcp').split(' — ')[1]}</button>
                              </div>
                            )}
                          </div>
                        );
                      })()}

                      {/* Discussions */}
                      <div style={s.section}>
                        <div style={s.sectionHeader}>
                          <MessageSquare size={14} /> <span style={s.sectionTitle}>Discussions</span>
                          <span style={s.count}>{allDiscussions.filter(d => d.project_id === proj.id).length}</span>
                        </div>
                        {allDiscussions.filter(d => d.project_id === proj.id).slice(0, 3).map(disc => (
                          <div key={disc.id} style={s.row}>
                            <div style={s.dot(true)} />
                            <div style={{ flex: 1 }}>
                              <span style={{ fontWeight: 600, fontSize: 12, display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                                {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00' }} />}
                                {disc.title}
                              </span>
                              <span style={{ marginLeft: 8, fontSize: 11, color: 'rgba(255,255,255,0.3)' }}>
                                {disc.message_count ?? disc.messages.length} msg · {disc.agent}
                              </span>
                            </div>
                            <button style={s.iconBtn} onClick={() => { setPage('discussions'); }}>
                              <ChevronRight size={12} />
                            </button>
                          </div>
                        ))}
                        <button
                          style={{ ...s.iconBtn, marginTop: 8, fontSize: 11, gap: 4 }}
                          onClick={() => { setDiscPrefill({ projectId: proj.id, title: '', prompt: '' }); setPage('discussions'); }}
                        >
                          <Plus size={12} /> {t('disc.newTitle')}
                        </button>
                      </div>

                      {/* AI Audit */}
                      <div style={s.section}>
                        <div style={s.sectionHeader}>
                          <FileCode size={14} /> <span style={s.sectionTitle}>AI Context</span>
                          <span style={s.count}>
                            {proj.audit_status === 'Validated' ? t('projects.status.valid') : validationInProgress ? t('projects.status.validating') : proj.audit_status === 'Audited' ? t('projects.status.auditOk') : proj.audit_status === 'TemplateInstalled' ? t('projects.status.template') : t('projects.status.none')}
                          </span>
                        </div>

                        {proj.audit_status === 'NoTemplate' && (
                          <div style={{ padding: '8px 0' }}>
                            <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', margin: '0 0 8px' }}>
                              {t('audit.noTemplate')}
                            </p>
                            <button
                              style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                              onClick={() => handleInstallTemplate(proj.id)}
                              disabled={installingTemplate === proj.id}
                            >
                              {installingTemplate === proj.id
                                ? <><Loader2 size={12} style={{ animation: 'spin 1s linear infinite' }} /> Installation...</>
                                : <><Download size={12} /> {t('audit.installTemplate')}</>
                              }
                            </button>
                          </div>
                        )}

                        {proj.audit_status === 'TemplateInstalled' && !auditState[proj.id]?.active && (
                          <div style={{ padding: '8px 0' }}>
                            <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 4 }}>
                              <AlertTriangle size={11} /> {t('audit.description')}
                            </p>
                            <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', margin: '0 0 8px' }}>
                              {t('audit.warning')}
                            </p>
                            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                              <select
                                style={{ ...auditSelectStyle, width: 'auto', minWidth: 140, fontSize: 12, padding: '6px 32px 6px 10px' }}
                                value={auditAgentChoice[proj.id] ?? agents.filter(isUsable)[0]?.agent_type ?? 'ClaudeCode'}
                                onChange={e => setAuditAgentChoice(prev => ({ ...prev, [proj.id]: e.target.value as AgentType }))}
                              >
                                {agents.filter(isUsable).map(a => (
                                  <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                                ))}
                                {agents.filter(isUsable).length === 0 && (
                                  <option value="" disabled>{t('disc.noAgent')}</option>
                                )}
                              </select>
                              <button
                                style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                onClick={() => handleLaunchAudit(proj.id)}
                                disabled={agents.filter(isUsable).length === 0}
                              >
                                <Play size={12} /> {t('audit.launch')}
                              </button>
                            </div>
                          </div>
                        )}

                        {auditState[proj.id]?.active && (
                          <div style={{ padding: '8px 0' }}>
                            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                              <Loader2 size={14} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />
                              <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)' }}>
                                {t('audit.step', auditState[proj.id].step, auditState[proj.id].totalSteps, auditState[proj.id].currentFile)}
                              </span>
                            </div>
                            <div style={{ height: 3, background: 'rgba(255,255,255,0.06)', borderRadius: 2, overflow: 'hidden' }}>
                              <div style={{
                                height: '100%',
                                width: `${(auditState[proj.id].step / auditState[proj.id].totalSteps) * 100}%`,
                                background: '#c8ff00',
                                borderRadius: 2,
                                transition: 'width 0.5s ease',
                              }} />
                            </div>
                          </div>
                        )}

                        {proj.audit_status === 'Audited' && !auditState[proj.id]?.active && (
                          <div style={{ padding: '8px 0' }}>
                            {validationInProgress ? (
                              <>
                                <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 4 }}>
                                  <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.validationInProgress', validationDisc.message_count ?? validationDisc.messages.length)}
                                </p>
                                <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', margin: '0 0 8px' }}>
                                  {t('audit.validationHint')}
                                </p>
                                <button
                                  style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                  onClick={() => { setPage('discussions'); }}
                                >
                                  <MessageSquare size={12} /> {t('audit.resumeValidation')}
                                </button>
                              </>
                            ) : (
                              <>
                                <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', margin: '0 0 8px' }}>
                                  {t('audit.readyToValidate')}
                                </p>
                                <button
                                  style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                  onClick={() => {
                                    setDiscPrefill({
                                      projectId: proj.id,
                                      title: 'Validation audit AI',
                                      prompt: `Voici le contexte AI du projet (dossier ai/). Analyse l'ensemble des fichiers ai/, identifie les zones d'ambiguite, les questions non resolues, les marqueurs <!-- TODO --> et les incoherences. Pose-moi tes questions une par une. Important : a chaque reponse de ma part, mets immediatement a jour les fichiers ai/ concernes avant de poser la question suivante — cela evite de repeter les memes questions et garde la documentation a jour en continu. Quand toutes mes reponses seront comprises, n'hesite pas a poser des questions de suivi. Une fois tout clarifie, termine ton message par la phrase exacte : "KRONN:VALIDATION_COMPLETE".`,
                                    });
                                    setPage('discussions');
                                  }}
                                >
                                  <ShieldCheck size={12} /> {t('audit.validate')}
                                </button>
                              </>
                            )}
                          </div>
                        )}

                        {proj.audit_status === 'Validated' && !auditState[proj.id]?.active && (
                          <div style={{ padding: '4px 0', fontSize: 11, color: 'rgba(200,255,0,0.5)', display: 'flex', alignItems: 'center', gap: 4 }}>
                            <ShieldCheck size={11} /> {t('audit.done')}
                          </div>
                        )}
                      </div>

                      <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 12, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,0.05)' }}>
                        <button style={s.dangerBtn} onClick={() => handleDeleteProject(proj.id)}>
                          <Trash2 size={12} /> {t('projects.delete')}
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}

            {/* Show more / less buttons */}
            {remainingCount > 0 && (
              <button
                style={{ display: 'block', width: '100%', padding: '10px 0', background: 'rgba(200,255,0,0.04)', border: '1px solid rgba(200,255,0,0.1)', borderRadius: 8, color: '#c8ff00', fontSize: 12, cursor: 'pointer', fontFamily: 'inherit', marginTop: 8 }}
                onClick={() => setProjectDisplayLimit(prev => prev + 20)}
              >
                {t('projects.showMore', remainingCount, remainingCount > 1 ? 's' : '', remainingCount > 1 ? 's' : '')}
              </button>
            )}
            {!projectSearch && projectDisplayLimit > 20 && remainingCount === 0 && sortedProjects.length > 20 && (
              <button
                style={{ display: 'block', width: '100%', padding: '8px 0', background: 'transparent', border: '1px solid rgba(255,255,255,0.06)', borderRadius: 8, color: 'rgba(255,255,255,0.3)', fontSize: 11, cursor: 'pointer', fontFamily: 'inherit', marginTop: 8 }}
                onClick={() => setProjectDisplayLimit(20)}
              >
                {t('projects.collapse')}
              </button>
            )}

            {displayProjects.length === 0 && (
              <div style={{ ...s.card(false), textAlign: 'center', padding: 40 }}>
                <Folder size={32} style={{ color: 'rgba(255,255,255,0.15)', marginBottom: 12 }} />
                <p style={{ color: 'rgba(255,255,255,0.4)', fontSize: 13 }}>
                  {projectSearch ? t('projects.emptySearch') : t('projects.empty')}
                </p>
              </div>
            )}
          </div>
          );
        })()}

        {/* ════════ MCPs ════════ */}
        {page === 'mcps' && (
          <McpPage projects={projects} mcpOverview={mcpOverview} mcpRegistry={mcpRegistry} refetchMcps={refetchMcps} />
        )}

        {/* ════════ WORKFLOWS ════════ */}
        {page === 'workflows' && (
          <WorkflowsPage projects={projects} installedAgentTypes={agents.filter(isUsable).map(a => a.agent_type)} agentAccess={agentAccess ?? undefined} />
        )}

        {/* ════════ DISCUSSIONS ════════ */}
        {page === 'discussions' && (
          <DiscussionsPage
            projects={projects}
            agents={agents}
            allDiscussions={allDiscussions}
            configLanguage={configLanguage ?? null}
            agentAccess={agentAccess ?? null}
            refetchDiscussions={refetchDiscussions}
            refetchProjects={refetch}
            onNavigate={(p) => setPage(p as Page)}
            prefill={discPrefill}
            onPrefillConsumed={handlePrefillConsumed}
            toast={toast}
            sendingMap={sendingMap}
            setSendingMap={setSendingMap}
            streamingMap={streamingMap}
            setStreamingMap={setStreamingMap}
            abortControllers={abortControllers}
            cleanupStream={cleanupStream}
            markDiscussionSeen={markDiscussionSeen}
            onActiveDiscussionChange={setActiveDiscussionId}
            initialActiveDiscussionId={activeDiscussionId}
            lastSeenMsgCount={lastSeenMsgCount}
          />
        )}

        {/* ════════ CONFIG ════════ */}
        {page === 'settings' && (
          <SettingsPage
            agents={agents}
            agentAccess={agentAccess ?? null}
            configLanguage={configLanguage ?? null}
            projects={projects}
            refetchAgents={refetchAgents}
            refetchAgentAccess={refetchAgentAccess}
            refetchLanguage={refetchLanguage}
            refetchProjects={refetch}
            refetchDiscussions={refetchDiscussions}
            onReset={onReset}
            toast={toast}
          />
        )}
      </main>
    </div>
  );
}

// ─── Styles ─────────────────────────────────────────────────────────────────

// Standalone styled select for audit agent chooser (was ds.selectStyled)
const auditSelectStyle = {
  width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
  borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
  cursor: 'pointer', appearance: 'none' as const,
  backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E")`,
  backgroundRepeat: 'no-repeat', backgroundPosition: 'right 10px center',
  paddingRight: 32,
} as const;

const s = {
  app: { minHeight: '100vh', background: '#0a0c10' } as const,
  nav: { display: 'flex', alignItems: 'center', gap: 4, padding: '10px 16px', borderBottom: '1px solid rgba(255,255,255,0.07)', background: '#12151c', position: 'sticky' as const, top: 0, zIndex: 50 },
  navBrand: { display: 'flex', alignItems: 'center', gap: 8, marginRight: 20 } as const,
  navTitle: { fontWeight: 700, fontSize: 14, letterSpacing: '-0.02em' } as const,
  navBtn: (active: boolean) => ({ padding: '7px 14px', borderRadius: 6, border: 'none', background: active ? 'rgba(200,255,0,0.1)' : 'transparent', color: active ? '#c8ff00' : 'rgba(255,255,255,0.45)', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: active ? 600 : 400, display: 'flex', alignItems: 'center', gap: 6 } as const),
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6 } as const,
  main: { maxWidth: 1000, margin: '0 auto', padding: '24px 20px' } as const,
  mainFull: { margin: 0, padding: '24px 20px', flex: 1 } as const,
  pageHeader: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 } as const,
  h1: { fontSize: 20, fontWeight: 700, letterSpacing: '-0.02em' } as const,
  meta: { color: 'rgba(255,255,255,0.4)', fontSize: 12, marginTop: 4 } as const,
  card: (active: boolean) => ({ background: '#12151c', border: `1px solid ${active ? 'rgba(255,255,255,0.12)' : 'rgba(255,255,255,0.07)'}`, borderRadius: 10, marginBottom: 12, transition: 'border-color 0.2s' } as const),
  cardHeader: { display: 'flex', alignItems: 'center', gap: 12, padding: '16px 20px', cursor: 'pointer' } as const,
  cardBody: { padding: '0 20px 20px', borderTop: '1px solid rgba(255,255,255,0.05)' } as const,
  projName: { fontWeight: 600, fontSize: 14 } as const,
  projPath: { fontSize: 11, color: 'rgba(255,255,255,0.25)', fontFamily: 'JetBrains Mono, monospace', marginTop: 2 } as const,
  projMeta: { display: 'flex', gap: 14, fontSize: 11, color: 'rgba(255,255,255,0.4)', flexShrink: 0 } as const,
  metaItem: { display: 'flex', alignItems: 'center', gap: 4 } as const,
  section: { marginTop: 16 } as const,
  sectionHeader: { display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10, color: 'rgba(255,255,255,0.6)', fontSize: 12 } as const,
  sectionTitle: { fontWeight: 600 } as const,
  sectionLabel: { fontSize: 11, fontWeight: 600, textTransform: 'uppercase' as const, letterSpacing: '0.05em', color: 'rgba(255,255,255,0.4)', marginBottom: 10 },
  count: { fontSize: 10, padding: '1px 6px', borderRadius: 10, background: 'rgba(255,255,255,0.06)', color: 'rgba(255,255,255,0.4)' } as const,
  row: { display: 'flex', alignItems: 'center', gap: 10, padding: '8px 12px', borderRadius: 6, background: 'rgba(255,255,255,0.02)', marginBottom: 4 } as const,
  dot: (on: boolean) => ({ width: 7, height: 7, borderRadius: '50%', background: on ? '#34d399' : 'rgba(255,255,255,0.15)', boxShadow: on ? '0 0 6px rgba(52,211,153,0.4)' : 'none', flexShrink: 0 } as const),
  badge: { display: 'inline-block', padding: '2px 8px', borderRadius: 4, fontSize: 10, background: 'rgba(255,255,255,0.06)', color: 'rgba(255,255,255,0.5)' } as const,
  aiBadge: { display: 'inline-block', padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(52,211,153,0.1)', color: 'rgba(52,211,153,0.7)', border: '1px solid rgba(52,211,153,0.15)' } as const,
  badgeGreen: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(200,255,0,0.1)', color: 'rgba(200,255,0,0.7)', border: '1px solid rgba(200,255,0,0.15)' } as const,
  badgeOrange: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(255,200,0,0.08)', color: 'rgba(255,200,0,0.6)', border: '1px solid rgba(255,200,0,0.12)' } as const,
  badgeGray: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(255,255,255,0.03)', color: 'rgba(255,255,255,0.2)', border: '1px solid rgba(255,255,255,0.05)' } as const,
  agentBadge: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(139,92,246,0.1)', color: 'rgba(139,92,246,0.7)', border: '1px solid rgba(139,92,246,0.15)' } as const,
  iconBtn: { background: 'none', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 4, padding: '4px 8px', color: 'rgba(255,255,255,0.5)', cursor: 'pointer', display: 'flex', alignItems: 'center', fontSize: 11 } as const,
  dangerBtn: { background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 6, padding: '6px 14px', color: '#ff4d6a', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit' } as const,
  installBtn: { padding: '6px 14px', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', border: '1px solid rgba(200,255,0,0.2)', borderRadius: 6, fontSize: 12, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'inherit' } as const,
  input: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', outline: 'none' } as const,
  mcpDropdown: { marginTop: 4, borderRadius: 8, border: '1px solid rgba(255,255,255,0.08)', background: '#181c26', overflow: 'hidden' } as const,
  mcpOption: { display: 'flex', alignItems: 'center', gap: 8, padding: '10px 12px', cursor: 'pointer', fontSize: 12, borderBottom: '1px solid rgba(255,255,255,0.04)' } as const,
  empty: { padding: 16, textAlign: 'center' as const, color: 'rgba(255,255,255,0.25)', fontSize: 12 },
  agentCard: { display: 'flex', alignItems: 'center', gap: 12, padding: '12px 16px', borderRadius: 8, background: 'rgba(255,255,255,0.03)', marginBottom: 6 } as const,
  originBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(100,180,255,0.1)', color: 'rgba(100,180,255,0.7)', border: '1px solid rgba(100,180,255,0.15)' } as const,
  updateBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(255,200,0,0.1)', color: '#ffc800', marginLeft: 6 } as const,
  badgeOk: { display: 'flex', alignItems: 'center', gap: 4, fontSize: 11, color: '#34d399', padding: '4px 10px', borderRadius: 20, background: 'rgba(52,211,153,0.1)' } as const,
  code: { fontSize: 11, fontFamily: 'JetBrains Mono, monospace', background: 'rgba(255,255,255,0.06)', padding: '2px 6px', borderRadius: 4 } as const,
  registryChip: { display: 'flex', alignItems: 'center', gap: 4, padding: '5px 10px', borderRadius: 6, background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.06)', fontSize: 11, color: 'rgba(255,255,255,0.5)' } as const,
  mcpCard: { display: 'flex', alignItems: 'flex-start', gap: 12, padding: '14px 16px', borderRadius: 8, background: 'rgba(255,255,255,0.03)', marginBottom: 8, border: '1px solid rgba(255,255,255,0.06)' } as const,
  sourceBadge: { fontSize: 10, fontWeight: 600, padding: '1px 6px', borderRadius: 4, background: 'rgba(200,255,0,0.08)', color: 'rgba(200,255,0,0.6)', border: '1px solid rgba(200,255,0,0.12)' } as const,
  projectChip: { display: 'flex', alignItems: 'center', gap: 3, padding: '2px 8px', borderRadius: 4, fontSize: 10, background: 'rgba(255,255,255,0.04)', color: 'rgba(255,255,255,0.4)', border: '1px solid rgba(255,255,255,0.06)' } as const,
  projectToggle: (active: boolean) => ({ display: 'flex', alignItems: 'center', gap: 4, padding: '4px 10px', borderRadius: 5, fontSize: 11, fontFamily: 'inherit', cursor: 'pointer', border: `1px solid ${active ? 'rgba(200,255,0,0.2)' : 'rgba(255,255,255,0.08)'}`, background: active ? 'rgba(200,255,0,0.06)' : 'rgba(255,255,255,0.02)', color: active ? 'rgba(200,255,0,0.8)' : 'rgba(255,255,255,0.35)' } as const),
};
