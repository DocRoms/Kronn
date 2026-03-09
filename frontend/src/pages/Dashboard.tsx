import { useState, useRef, useEffect, useCallback } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { projects as projectsApi, mcps as mcpsApi, agents as agentsApi, discussions as discussionsApi, config as configApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import type { Project, AgentDetection, Discussion, AgentType } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { UI_LOCALES } from '../lib/i18n';
import { McpPage } from './McpPage';
import { WorkflowsPage } from './WorkflowsPage';
import {
  Folder, Server, ChevronRight, Cpu, Workflow,
  Plus, Trash2, Search, Zap, Settings, Eye,
  Download, Upload, Check, Loader2, RefreshCw,
  MessageSquare, Send, X, Key, AlertTriangle, Save, Users,
  StopCircle, RotateCcw, Pencil, HardDrive, Play, FileCode, ShieldCheck,
} from 'lucide-react';

type Page = 'projects' | 'mcps' | 'workflows' | 'discussions' | 'settings';

interface DashboardProps {
  onReset: () => void;
}

const isHiddenPath = (path: string) => path.split('/').some(s => s.startsWith('.'));

const isAiReady = (p: Project) => p.audit_status !== 'NoTemplate';

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

const AGENT_COLORS: Record<string, string> = {
  'ClaudeCode': '#c8ff00',
  'Claude Code': '#c8ff00',
  'Codex': '#00d4ff',
  'Vibe': '#ff6b6b',
};

const agentColor = (agentType: string | null | undefined): string =>
  AGENT_COLORS[agentType ?? ''] ?? '#8b5cf6';

// Sort score for project readiness
const readinessScore = (p: Project) => {
  return p.ai_config.detected ? 0 : 1;
};

export function Dashboard({ onReset }: DashboardProps) {
  const { t, locale, setLocale } = useT();
  const [page, setPage] = useState<Page>('projects');
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  const [installing, setInstalling] = useState<string | null>(null);
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(null);
  const [showNewDiscussion, setShowNewDiscussion] = useState(false);
  const [newDiscTitle, setNewDiscTitle] = useState('');
  const [newDiscAgent, setNewDiscAgent] = useState<AgentType | ''>('');
  const [newDiscProjectId, setNewDiscProjectId] = useState<string>('');
  const [newDiscPrompt, setNewDiscPrompt] = useState('');
  const [newDiscPrefilled, setNewDiscPrefilled] = useState(false);
  const [chatInput, setChatInput] = useState('');
  const [sendingMap, setSendingMap] = useState<Record<string, boolean>>({});
  const [streamingMap, setStreamingMap] = useState<Record<string, string>>({});
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [collapsedDiscGroups, setCollapsedDiscGroups] = useState<Set<string>>(new Set());
  const [editingText, setEditingText] = useState('');
  const abortControllers = useRef<Record<string, AbortController>>({});
  const [showDebatePopover, setShowDebatePopover] = useState(false);
  const [debateAgents, setDebateAgents] = useState<AgentType[]>([]);
  // Orchestration live state: per-discussion, tracks current agent streaming and round
  const [orchState, setOrchState] = useState<Record<string, {
    active: boolean;
    round: number | string;
    totalRounds: number;
    currentAgent: string | null;
    agentStreams: { agent: string; agentType: string; round: number | string; text: string; done: boolean }[];
    systemMessages: string[];
  }>>({});
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [lastSeenMsgCount, setLastSeenMsgCount] = useState<Record<string, number>>(() => {
    try { return JSON.parse(localStorage.getItem('kronn:lastSeenMsgCount') ?? '{}'); } catch { return {}; }
  });
  const chatInputRef = useRef<HTMLInputElement>(null);
  const [tokenInputs, setTokenInputs] = useState({ anthropic: '', openai: '' });
  const [tokenSaving, setTokenSaving] = useState(false);
  const [tokenSaved, setTokenSaved] = useState(false);
  const chatEndRef = useRef<HTMLDivElement>(null);
  // AI audit state
  const [installingTemplate, setInstallingTemplate] = useState<string | null>(null);
  const [auditState, setAuditState] = useState<Record<string, {
    active: boolean;
    step: number;
    totalSteps: number;
    currentFile: string;
  }>>({});

  const { data: projectList, refetch } = useApi(() => projectsApi.list(), []);
  const { data: registry } = useApi(() => mcpsApi.registry(), []);
  const { data: mcpOverviewData, refetch: refetchMcps } = useApi(() => mcpsApi.overview(), []);
  const { data: agentList, refetch: refetchAgents } = useApi(() => agentsApi.detect(), []);
  const { data: discussionList, refetch: refetchDiscussions } = useApi(() => discussionsApi.list(), []);
  const { data: tokenConfig, refetch: refetchTokens } = useApi(() => configApi.getTokens(), []);
  const { data: configLanguage, refetch: refetchLanguage } = useApi(() => configApi.getLanguage(), []);
  const { data: dbInfo, refetch: refetchDbInfo } = useApi(() => configApi.dbInfo(), []);
  const { data: agentAccess, refetch: refetchAgentAccess } = useApi(() => configApi.getAgentAccess(), []);

  const projects = projectList ?? [];
  const mcpRegistry = registry ?? [];
  const mcpOverview = mcpOverviewData ?? { servers: [], configs: [], customized_contexts: [] };
  const agents = agentList ?? [];
  const allDiscussions = discussionList ?? [];

  const activeDiscussion = allDiscussions.find(d => d.id === activeDiscussionId) ?? null;

  const sending = activeDiscussionId ? !!sendingMap[activeDiscussionId] : false;
  const streamingText = activeDiscussionId ? (streamingMap[activeDiscussionId] ?? '') : '';

  const LANGUAGES: { code: string; label: string; flag: string }[] = [
    { code: 'fr', label: 'Francais', flag: 'FR' },
    { code: 'en', label: 'English', flag: 'EN' },
    { code: 'es', label: 'Español', flag: 'ES' },
    { code: 'zh', label: '中文', flag: 'ZH' },
    { code: 'br', label: 'Brezhoneg', flag: 'BR' },
  ];

  const ALL_AGENT_MENTIONS: { trigger: string; type: AgentType; label: string }[] = [
    { trigger: '@claude', type: 'ClaudeCode', label: 'Claude Code' },
    { trigger: '@codex', type: 'Codex', label: 'Codex' },
    { trigger: '@vibe', type: 'Vibe', label: 'Vibe' },
  ];
  const activeAgentTypes = new Set(agents.filter(a => a.installed && a.enabled).map(a => a.agent_type));
  const AGENT_MENTIONS = ALL_AGENT_MENTIONS.filter(m => activeAgentTypes.has(m.type));

  // Track "last seen" message count for unread badges (persisted in localStorage)
  // Only update when the user explicitly selects a discussion (not on data refresh)
  const markDiscussionSeen = useCallback((discId: string, msgCount: number) => {
    setLastSeenMsgCount(prev => {
      const next = { ...prev, [discId]: msgCount };
      localStorage.setItem('kronn:lastSeenMsgCount', JSON.stringify(next));
      return next;
    });
  }, []);

  // Mark active discussion as seen when user selects it or when messages arrive while viewing
  useEffect(() => {
    if (activeDiscussionId && activeDiscussion && !sendingMap[activeDiscussionId]) {
      // Only mark as seen when the agent is NOT currently streaming
      // This way, the badge stays until the user actually "sees" the final result
      markDiscussionSeen(activeDiscussionId, activeDiscussion.messages.length);
    }
  }, [activeDiscussionId, activeDiscussion?.messages.length, sendingMap]);

  // Poll discussions every 10s to catch agent responses that complete in the background
  useEffect(() => {
    const interval = setInterval(() => {
      refetchDiscussions();
    }, 10000);
    return () => clearInterval(interval);
  }, [refetchDiscussions]);

  // Compute total unseen count for tab indicator
  const totalUnseen = allDiscussions.reduce((acc, disc) => {
    const unseen = disc.messages.length - (lastSeenMsgCount[disc.id] ?? 0);
    return acc + (unseen > 0 && disc.id !== activeDiscussionId ? unseen : 0);
  }, 0);

  // Update document title with unread indicator
  useEffect(() => {
    document.title = totalUnseen > 0 ? `(${totalUnseen}) Kronn` : 'Kronn';
  }, [totalUnseen]);

  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [activeDiscussion?.messages.length, streamingText]);

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
      await projectsApi.auditStream(projectId, { agent: 'ClaudeCode' }, {
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

  const handleInstallAgent = async (agent: AgentDetection) => {
    setInstalling(agent.name);
    try {
      await agentsApi.install(agent.agent_type);
      refetchAgents();
    } catch {
      // silently fail
    } finally {
      setInstalling(null);
    }
  };

  return (
    <div style={s.app}>
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
            <Icon size={14} /> {label}
            {id === 'discussions' && totalUnseen > 0 && (
              <span style={{
                position: 'absolute', top: 2, right: 2,
                background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center',
                lineHeight: '12px',
              }}>{totalUnseen}</span>
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
                          <span style={{ ...s.badgeOrange, cursor: 'pointer' }} onClick={(e) => { e.stopPropagation(); setActiveDiscussionId(validationDisc.id); setPage('discussions'); }}>
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
                                {disc.messages.length} msg · {disc.agent}
                              </span>
                            </div>
                            <button style={s.iconBtn} onClick={() => { setActiveDiscussionId(disc.id); setPage('discussions'); }}>
                              <ChevronRight size={12} />
                            </button>
                          </div>
                        ))}
                        <button
                          style={{ ...s.iconBtn, marginTop: 8, fontSize: 11, gap: 4 }}
                          onClick={() => { setPage('discussions'); setShowNewDiscussion(true); setNewDiscProjectId(proj.id); }}
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
                            <button
                              style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                              onClick={() => handleLaunchAudit(proj.id)}
                            >
                              <Play size={12} /> {t('audit.launch')}
                            </button>
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
                                  <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.validationInProgress', validationDisc.messages.length)}
                                </p>
                                <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', margin: '0 0 8px' }}>
                                  {t('audit.validationHint')}
                                </p>
                                <button
                                  style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                  onClick={() => { setActiveDiscussionId(validationDisc.id); setPage('discussions'); }}
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
                                    setPage('discussions');
                                    setShowNewDiscussion(true);
                                    setNewDiscPrefilled(true);
                                    setNewDiscProjectId(proj.id);
                                    setNewDiscTitle('Validation audit AI');
                                    setNewDiscPrompt(`Voici le contexte AI du projet (dossier ai/). Analyse l'ensemble des fichiers ai/, identifie les zones d'ambiguite, les questions non resolues, les marqueurs <!-- TODO --> et les incoherences. Pose-moi tes questions une par une. Important : a chaque reponse de ma part, mets immediatement a jour les fichiers ai/ concernes avant de poser la question suivante — cela evite de repeter les memes questions et garde la documentation a jour en continu. Quand toutes mes reponses seront comprises, n'hesite pas a poser des questions de suivi. Une fois tout clarifie, termine ton message par la phrase exacte : "KRONN:VALIDATION_COMPLETE".`);
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
          <WorkflowsPage projects={projects} installedAgentTypes={agents.filter(a => a.installed && a.enabled).map(a => a.agent_type)} />
        )}

        {/* ════════ DISCUSSIONS ════════ */}
        {page === 'discussions' && (() => {
          const installedAgents = agents.filter(a => a.installed && a.enabled);
          // Auto-select first installed agent if current selection is invalid
          if (installedAgents.length > 0 && !installedAgents.some(a => a.agent_type === newDiscAgent)) {
            setNewDiscAgent(installedAgents[0].agent_type);
          }
          // Group discussions by project (null = global)
          const discByProject = new Map<string | null, Discussion[]>();
          for (const d of allDiscussions) {
            const key = d.project_id ?? null;
            const list = discByProject.get(key) ?? [];
            list.push(d);
            discByProject.set(key, list);
          }

          const cleanupStream = (discId: string) => {
            setSendingMap(prev => ({ ...prev, [discId]: false }));
            setStreamingMap(prev => { const n = { ...prev }; delete n[discId]; return n; });
            delete abortControllers.current[discId];
            refetchDiscussions();
          };

          const handleCreateDiscussion = async () => {
            if (!newDiscPrompt.trim() || !newDiscAgent) return;
            const prompt = newDiscPrompt.trim();
            const title = newDiscTitle.trim() || prompt.slice(0, 60);
            const disc = await discussionsApi.create({
              project_id: newDiscProjectId || null,
              title,
              agent: newDiscAgent as AgentType,
              language: configLanguage ?? 'fr',
              initial_prompt: prompt,
            });
            setShowNewDiscussion(false);
            setNewDiscTitle('');
            setNewDiscPrompt('');
            setNewDiscPrefilled(false);
            setActiveDiscussionId(disc.id);
            refetchDiscussions();

            const discId = disc.id;
            const controller = new AbortController();
            abortControllers.current[discId] = controller;
            setSendingMap(prev => ({ ...prev, [discId]: true }));
            setStreamingMap(prev => ({ ...prev, [discId]: '' }));
            await discussionsApi.runAgent(
              discId,
              (text) => setStreamingMap(prev => ({ ...prev, [discId]: prev[discId] ? prev[discId] + '\n' + text : text })),
              () => cleanupStream(discId),
              (error) => { console.error('Agent error:', error); cleanupStream(discId); },
              controller.signal,
            );
          };

          const parseMention = (text: string): { targetAgent?: AgentType } => {
            for (const m of AGENT_MENTIONS) {
              if (text.toLowerCase().startsWith(m.trigger + ' ') || text.toLowerCase() === m.trigger) {
                return { targetAgent: m.type };
              }
            }
            return {};
          };

          const handleSendMessage = async () => {
            if (!activeDiscussionId || !chatInput.trim() || sending) return;
            const discId = activeDiscussionId;
            const msg = chatInput.trim();
            const { targetAgent } = parseMention(msg);
            setChatInput('');
            setMentionQuery(null);
            const controller = new AbortController();
            abortControllers.current[discId] = controller;
            setStreamingMap(prev => ({ ...prev, [discId]: '' }));

            await discussionsApi.sendMessageStream(
              discId,
              { content: msg, target_agent: targetAgent },
              (text) => setStreamingMap(prev => ({ ...prev, [discId]: prev[discId] ? prev[discId] + '\n' + text : text })),
              () => cleanupStream(discId),
              (error) => { console.error('Agent error:', error); cleanupStream(discId); },
              controller.signal,
              () => {
                // Backend has added the user message — refetch to show it, then show streaming
                refetchDiscussions();
                setSendingMap(prev => ({ ...prev, [discId]: true }));
              },
            );
          };

          const handleStop = () => {
            if (!activeDiscussionId) return;
            const discId = activeDiscussionId;
            const controller = abortControllers.current[discId];
            if (controller) controller.abort();
            // Ensure UI clears even if SSE abort callback doesn't fire
            cleanupStream(discId);
          };

          const handleRetry = async () => {
            if (!activeDiscussionId || sending) return;
            const discId = activeDiscussionId;
            // Delete trailing agent messages, then re-run
            await discussionsApi.deleteLastAgentMessages(discId);
            await refetchDiscussions();
            const controller = new AbortController();
            abortControllers.current[discId] = controller;
            setSendingMap(prev => ({ ...prev, [discId]: true }));
            setStreamingMap(prev => ({ ...prev, [discId]: '' }));
            await discussionsApi.runAgent(
              discId,
              (text) => setStreamingMap(prev => ({ ...prev, [discId]: prev[discId] ? prev[discId] + '\n' + text : text })),
              () => cleanupStream(discId),
              (error) => { console.error('Agent error:', error); cleanupStream(discId); },
              controller.signal,
            );
          };

          const handleEditMessage = async () => {
            if (!activeDiscussionId || !editingMsgId || !editingText.trim() || sending) return;
            const discId = activeDiscussionId;
            // Delete trailing agent messages, edit the user message, then re-run
            await discussionsApi.deleteLastAgentMessages(discId);
            await discussionsApi.editLastUserMessage(discId, editingText.trim());
            setEditingMsgId(null);
            setEditingText('');
            await refetchDiscussions();
            const controller = new AbortController();
            abortControllers.current[discId] = controller;
            setSendingMap(prev => ({ ...prev, [discId]: true }));
            setStreamingMap(prev => ({ ...prev, [discId]: '' }));
            await discussionsApi.runAgent(
              discId,
              (text) => setStreamingMap(prev => ({ ...prev, [discId]: prev[discId] ? prev[discId] + '\n' + text : text })),
              () => cleanupStream(discId),
              (error) => { console.error('Agent error:', error); cleanupStream(discId); },
              controller.signal,
            );
          };

          const handleOrchestrate = async () => {
            if (!activeDiscussionId || debateAgents.length < 2) return;
            const discId = activeDiscussionId;
            const controller = new AbortController();
            abortControllers.current[discId] = controller;
            setShowDebatePopover(false);
            setSendingMap(prev => ({ ...prev, [discId]: true }));
            setOrchState(prev => ({
              ...prev,
              [discId]: { active: true, round: 0, totalRounds: 3, currentAgent: null, agentStreams: [], systemMessages: [] },
            }));

            await discussionsApi.orchestrate(discId, { agents: debateAgents, max_rounds: 3 }, {
              onSystem: (text) => {
                setOrchState(prev => {
                  const s = prev[discId];
                  return s ? { ...prev, [discId]: { ...s, systemMessages: [...s.systemMessages, text] } } : prev;
                });
              },
              onRound: (round, total) => {
                setOrchState(prev => {
                  const s = prev[discId];
                  return s ? { ...prev, [discId]: { ...s, round, totalRounds: total } } : prev;
                });
              },
              onAgentStart: (agent, agentType, round) => {
                setOrchState(prev => {
                  const s = prev[discId];
                  if (!s) return prev;
                  return { ...prev, [discId]: {
                    ...s, currentAgent: agent,
                    agentStreams: [...s.agentStreams, { agent, agentType, round, text: '', done: false }],
                  }};
                });
              },
              onChunk: (text, agent, _agentType, _round) => {
                setOrchState(prev => {
                  const s = prev[discId];
                  if (!s) return prev;
                  const streams = [...s.agentStreams];
                  const last = [...streams].reverse().find((st: typeof streams[0]) => st.agent === agent && !st.done);
                  if (last) last.text = last.text ? last.text + '\n' + text : text;
                  return { ...prev, [discId]: { ...s, agentStreams: streams } };
                });
              },
              onAgentDone: (agent) => {
                setOrchState(prev => {
                  const s = prev[discId];
                  if (!s) return prev;
                  const streams = s.agentStreams.map(st =>
                    st.agent === agent && !st.done ? { ...st, done: true } : st
                  );
                  return { ...prev, [discId]: { ...s, currentAgent: null, agentStreams: streams } };
                });
              },
              onDone: () => {
                setSendingMap(prev => ({ ...prev, [discId]: false }));
                delete abortControllers.current[discId];
                setOrchState(prev => {
                  const s = prev[discId];
                  return s ? { ...prev, [discId]: { ...s, active: false, currentAgent: null } } : prev;
                });
                refetchDiscussions();
              },
              onError: (error) => {
                console.error('Orchestration error:', error);
                setSendingMap(prev => ({ ...prev, [discId]: false }));
                delete abortControllers.current[discId];
                setOrchState(prev => {
                  const s = prev[discId];
                  return s ? { ...prev, [discId]: { ...s, active: false } } : prev;
                });
                refetchDiscussions();
              },
            }, controller.signal);
          };

          return (
          <div style={{ display: 'flex', height: 'calc(100vh - 56px)', margin: '-24px -20px', overflow: 'hidden' }}>
            {/* Sidebar */}
            <div style={ds.sidebar}>
              <div style={ds.sidebarHeader}>
                <span style={{ fontWeight: 600, fontSize: 13 }}>Discussions</span>
                <button style={s.scanBtn} onClick={() => { setShowNewDiscussion(true); }}>
                  <Plus size={12} /> {t('disc.new')}
                </button>
              </div>

              {/* Discussion list grouped by project */}
              <div style={ds.sidebarList}>
                {/* Global discussions (no project) */}
                {(() => {
                  const globalDiscs = discByProject.get(null) ?? [];
                  if (globalDiscs.length === 0) return null;
                  const isCollapsed = collapsedDiscGroups.has('__global__');
                  return (
                    <div>
                      <div
                        style={{ ...ds.projectGroup, borderTop: 'none', cursor: 'pointer', userSelect: 'none' as const }}
                        onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete('__global__') : n.add('__global__'); return n; })}
                      >
                        <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                        <MessageSquare size={10} /> {t('disc.general')}
                        <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{globalDiscs.length}</span>
                      </div>
                      {!isCollapsed && globalDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                        <button
                          key={disc.id}
                          style={ds.discItem(disc.id === activeDiscussionId)}
                          onClick={() => { setActiveDiscussionId(disc.id); markDiscussionSeen(disc.id, disc.messages.length); }}
                        >
                          <div style={{ flex: 1, minWidth: 0 }}>
                            <div style={{ fontSize: 12, fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', display: 'flex', alignItems: 'center', gap: 4 }}>
                              {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                              {disc.title}
                              {(() => {
                                const unseen = disc.messages.length - (lastSeenMsgCount[disc.id] ?? 0);
                                return unseen > 0 && disc.id !== activeDiscussionId ? (
                                  <span style={{
                                    background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                                    borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center', flexShrink: 0,
                                  }}>{unseen}</span>
                                ) : null;
                              })()}
                            </div>
                            <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4 }}>
                              {sendingMap[disc.id] && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />}
                              {(disc.participants?.length ?? 0) > 1 && (
                                <Users size={8} style={{ color: '#8b5cf6' }} />
                              )}
                              {disc.messages.length} msg · {disc.agent}
                            </div>
                          </div>
                        </button>
                      ))}
                    </div>
                  );
                })()}

                {/* Project discussions */}
                {projects.filter(p => !isHiddenPath(p.path)).map(proj => {
                  const projDiscs = discByProject.get(proj.id) ?? [];
                  if (projDiscs.length === 0) return null;
                  const isCollapsed = collapsedDiscGroups.has(proj.id);
                  return (
                    <div key={proj.id}>
                      <div
                        style={{ ...ds.projectGroup, cursor: 'pointer', userSelect: 'none' as const }}
                        onClick={() => setCollapsedDiscGroups(prev => { const n = new Set(prev); isCollapsed ? n.delete(proj.id) : n.add(proj.id); return n; })}
                      >
                        <ChevronRight size={10} style={{ transform: isCollapsed ? 'none' : 'rotate(90deg)', transition: 'transform 0.15s' }} />
                        <Folder size={10} /> {proj.name}
                        <span style={{ fontWeight: 400, opacity: 0.5, marginLeft: 'auto' }}>{projDiscs.length}</span>
                      </div>
                      {!isCollapsed && projDiscs.sort((a, b) => b.updated_at.localeCompare(a.updated_at)).map(disc => (
                        <button
                          key={disc.id}
                          style={ds.discItem(disc.id === activeDiscussionId)}
                          onClick={() => { setActiveDiscussionId(disc.id); markDiscussionSeen(disc.id, disc.messages.length); }}
                        >
                          <div style={{ flex: 1, minWidth: 0 }}>
                            <div style={{ fontSize: 12, fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', display: 'flex', alignItems: 'center', gap: 4 }}>
                              {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                              {disc.title}
                              {(() => {
                                const unseen = disc.messages.length - (lastSeenMsgCount[disc.id] ?? 0);
                                return unseen > 0 && disc.id !== activeDiscussionId ? (
                                  <span style={{
                                    background: '#c8ff00', color: '#0a0c10', fontSize: 8, fontWeight: 800,
                                    borderRadius: 6, padding: '1px 5px', minWidth: 14, textAlign: 'center', flexShrink: 0,
                                  }}>{unseen}</span>
                                ) : null;
                              })()}
                            </div>
                            <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 2, display: 'flex', alignItems: 'center', gap: 4 }}>
                              {sendingMap[disc.id] && <Loader2 size={8} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />}
                              {(disc.participants?.length ?? 0) > 1 && (
                                <Users size={8} style={{ color: '#8b5cf6' }} />
                              )}
                              {disc.messages.length} msg · {disc.agent}
                            </div>
                          </div>
                        </button>
                      ))}
                    </div>
                  );
                })}

                {allDiscussions.length === 0 && !showNewDiscussion && (
                  <div style={{ padding: 24, textAlign: 'center', color: 'rgba(255,255,255,0.25)', fontSize: 12, whiteSpace: 'pre-line' }}>
                    {t('disc.empty')}
                  </div>
                )}
              </div>
            </div>

            {/* Main area */}
            <div style={ds.chatArea}>
              {/* New discussion form */}
              {showNewDiscussion && (
                <div style={ds.newDiscOverlay}>
                  <div
                    style={ds.newDiscCard}
                    onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && newDiscPrompt.trim()) handleCreateDiscussion(); }}
                  >
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 }}>
                      <span style={{ fontWeight: 700, fontSize: 15, color: '#e8eaed' }}>{t('disc.newTitle')}</span>
                      <button style={s.iconBtn} onClick={() => { setShowNewDiscussion(false); setNewDiscPrefilled(false); }}><X size={14} /></button>
                    </div>

                    <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, marginBottom: 12 }}>
                      <div>
                        <label style={ds.label}>{t('disc.project')}</label>
                        <select style={{ ...ds.selectStyled, ...(newDiscPrefilled ? { opacity: 0.5, pointerEvents: 'none' as const } : {}) }} value={newDiscProjectId} onChange={e => setNewDiscProjectId(e.target.value)} disabled={newDiscPrefilled}>
                          <option value="">{t('disc.noProject')}</option>
                          {projects.filter(p => !isHiddenPath(p.path)).map(p => (
                            <option key={p.id} value={p.id}>{p.name}</option>
                          ))}
                        </select>
                      </div>
                      <div>
                        <label style={ds.label}>{t('disc.agent')}</label>
                        <select style={ds.selectStyled} value={newDiscAgent} onChange={e => setNewDiscAgent(e.target.value as AgentType)}>
                          {installedAgents.map(a => (
                            <option key={a.name} value={a.agent_type}>{a.name}</option>
                          ))}
                          {installedAgents.length === 0 && (
                            <option value="" disabled>{t('disc.noAgent')}</option>
                          )}
                        </select>
                      </div>
                    </div>

                    <label style={ds.label}>{t('disc.title')}</label>
                    <input
                      style={{ ...ds.inputStyled, ...(newDiscPrefilled ? { opacity: 0.5, cursor: 'not-allowed' } : {}) }}
                      placeholder={t('disc.titlePlaceholder')}
                      value={newDiscTitle}
                      onChange={e => !newDiscPrefilled && setNewDiscTitle(e.target.value)}
                      readOnly={newDiscPrefilled}
                    />

                    <label style={{ ...ds.label, marginTop: 12 }}>{t('disc.prompt')}</label>
                    <textarea
                      style={{ ...ds.textareaStyled, ...(newDiscPrefilled ? { opacity: 0.5, cursor: 'not-allowed' } : {}) }}
                      placeholder={t('disc.promptPlaceholder')}
                      value={newDiscPrompt}
                      onChange={e => !newDiscPrefilled && setNewDiscPrompt(e.target.value)}
                      readOnly={newDiscPrefilled}
                      rows={4}
                      autoFocus={!newDiscPrefilled}
                    />

                    {/* Warnings for validation discussion */}
                    {newDiscPrefilled && (
                      <div style={{ marginTop: 12, padding: '10px 12px', borderRadius: 8, background: 'rgba(255,200,0,0.06)', border: '1px solid rgba(255,200,0,0.12)' }}>
                        <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 6px', display: 'flex', alignItems: 'center', gap: 4 }}>
                          <AlertTriangle size={11} /> {t('disc.auditWarn')}
                        </p>
                        <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', margin: 0 }}>
                          {t('disc.auditHint')}
                        </p>
                      </div>
                    )}

                    <button
                      style={{
                        marginTop: 16, width: '100%', padding: '11px 16px', borderRadius: 8,
                        border: 'none', background: newDiscPrompt.trim() ? '#c8ff00' : 'rgba(255,255,255,0.06)',
                        color: newDiscPrompt.trim() ? '#0a0c10' : 'rgba(255,255,255,0.25)',
                        fontWeight: 700, fontSize: 13, fontFamily: 'inherit', cursor: 'pointer',
                        display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8,
                        transition: 'all 0.15s',
                      }}
                      onClick={handleCreateDiscussion}
                      disabled={!newDiscPrompt.trim() || !newDiscAgent}
                    >
                      <MessageSquare size={14} /> {t('disc.start')}
                      <span style={{ fontSize: 10, opacity: 0.6, marginLeft: 4 }}>Ctrl+Enter</span>
                    </button>
                  </div>
                </div>
              )}

              {/* Active discussion chat */}
              {activeDiscussion && !showNewDiscussion ? (
                <>
                  {/* Chat header */}
                  <div style={ds.chatHeader}>
                    <div style={{ flex: 1 }}>
                      <div style={{ fontWeight: 600, fontSize: 14, display: 'flex', alignItems: 'center', gap: 6 }}>
                        {isValidationDisc(activeDiscussion.title) && <ShieldCheck size={14} style={{ color: '#c8ff00', flexShrink: 0 }} />}
                        {activeDiscussion.title}
                      </div>
                      <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.35)', marginTop: 2 }}>
                        {activeDiscussion.project_id ? (projects.find(p => p.id === activeDiscussion.project_id)?.name ?? '?') : t('disc.general')} · {activeDiscussion.agent}
                      </div>
                    </div>
                    <button
                      style={{ ...s.iconBtn, color: '#ff4d6a' }}
                      onClick={async () => {
                        await discussionsApi.delete(activeDiscussion.id);
                        setActiveDiscussionId(null);
                        refetchDiscussions();
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </div>

                  {/* Messages */}
                  <div style={ds.messages}>
                    {activeDiscussion.messages.map((msg, idx) => {
                      const msgs = activeDiscussion.messages;
                      const isLastUser = msg.role === 'User' && !msgs.slice(idx + 1).some(m => m.role === 'User');
                      const isLastAgent = msg.role === 'Agent' && idx === msgs.length - 1;
                      const isEditing = editingMsgId === msg.id;

                      return (
                      <div key={msg.id} style={ds.msgRow(msg.role === 'User')}>
                        <div style={{
                          ...ds.msgBubble(msg.role === 'User'),
                          ...(msg.role === 'System' ? { borderColor: 'rgba(255,77,106,0.3)', background: 'rgba(255,77,106,0.06)' } : {}),
                        }}>
                          {msg.role === 'Agent' && (
                            <div style={{ ...ds.msgAgent, color: agentColor(msg.agent_type ?? activeDiscussion.agent) }}>
                              <Cpu size={10} /> {msg.agent_type ?? activeDiscussion.agent}
                            </div>
                          )}
                          {msg.role === 'System' && (
                            <div style={{ ...ds.msgAgent, color: '#ff4d6a' }}>
                              <AlertTriangle size={10} /> {t('disc.system')}
                            </div>
                          )}
                          {isEditing ? (
                            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                              <textarea
                                value={editingText}
                                onChange={e => setEditingText(e.target.value)}
                                onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); handleEditMessage(); } }}
                                style={{
                                  width: '100%', minHeight: 60, padding: 8, borderRadius: 6,
                                  background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(200,255,0,0.3)',
                                  color: '#e8eaed', fontFamily: 'inherit', fontSize: 12, resize: 'vertical',
                                }}
                                autoFocus
                              />
                              <div style={{ display: 'flex', gap: 6, justifyContent: 'flex-end' }}>
                                <button
                                  style={{ ...s.iconBtn, fontSize: 11, padding: '4px 10px', color: 'rgba(255,255,255,0.4)' }}
                                  onClick={() => { setEditingMsgId(null); setEditingText(''); }}
                                >
                                  {t('disc.cancel')}
                                </button>
                                <button
                                  style={{ ...s.scanBtn, fontSize: 11, padding: '4px 10px' }}
                                  onClick={handleEditMessage}
                                  disabled={!editingText.trim()}
                                >
                                  <Send size={10} /> {t('disc.resend')}
                                  <span style={{ fontSize: 9, opacity: 0.5, marginLeft: 4 }}>Ctrl+Enter</span>
                                </button>
                              </div>
                            </div>
                          ) : (
                            <MarkdownContent content={msg.content} />
                          )}
                          {/api.?key|invalid.*key|key.*not.*config|authenticat|unauthori|login|sign.?in/i.test(msg.content) && (
                            <div style={{ display: 'flex', gap: 8, marginTop: 8, flexWrap: 'wrap' }}>
                              <button
                                style={{ ...s.scanBtn, fontSize: 11, padding: '5px 12px' }}
                                onClick={() => setPage('settings')}
                              >
                                <Key size={11} /> {t('disc.overrideKey')}
                              </button>
                              <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', alignSelf: 'center' }}>
                                {t('disc.orCheckAgent')}
                              </span>
                            </div>
                          )}
                          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginTop: 4 }}>
                            <div style={ds.msgTime}>
                              {new Date(msg.timestamp).toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' })}
                            </div>
                            {!sending && !isEditing && (isLastUser || isLastAgent) && (
                              <div style={{ display: 'flex', gap: 4 }}>
                                {isLastUser && (
                                  <button
                                    style={{ ...s.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }}
                                    onClick={() => { setEditingMsgId(msg.id); setEditingText(msg.content); }}
                                    title={t('disc.editResend')}
                                  >
                                    <Pencil size={10} />
                                  </button>
                                )}
                                {isLastAgent && (
                                  <button
                                    style={{ ...s.iconBtn, padding: '2px 6px', fontSize: 10, color: 'rgba(255,255,255,0.3)' }}
                                    onClick={handleRetry}
                                    title={t('disc.retryResponse')}
                                  >
                                    <RotateCcw size={10} />
                                  </button>
                                )}
                              </div>
                            )}
                          </div>
                        </div>
                      </div>
                      );
                    })}

                    {/* Streaming: single agent mode */}
                    {sending && !orchState[activeDiscussion.id]?.active && (
                      <div style={ds.msgRow(false)}>
                        <div style={ds.msgBubble(false)}>
                          <div style={{ ...ds.msgAgent, color: agentColor(activeDiscussion.agent) }}>
                            <Cpu size={10} /> {activeDiscussion.agent}
                            <Loader2 size={10} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />
                          </div>
                          {streamingText ? (
                            <MarkdownContent content={streamingText} />
                          ) : (
                            <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.3)', fontStyle: 'italic' }}>
                              {t('disc.running')}
                            </div>
                          )}
                        </div>
                      </div>
                    )}

                    {/* Streaming: orchestration mode */}
                    {orchState[activeDiscussion.id] && (() => {
                      const orch = orchState[activeDiscussion.id];
                      return (
                        <>
                          {orch.agentStreams.map((as_, i) => (
                            <div key={i} style={ds.msgRow(false)}>
                              <div style={{
                                ...ds.msgBubble(false),
                                borderLeft: `3px solid ${agentColor(as_.agentType || as_.agent)}`,
                              }}>
                                <div style={{
                                  display: 'flex', alignItems: 'center', gap: 4, fontSize: 10,
                                  fontWeight: 600, color: agentColor(as_.agentType || as_.agent), marginBottom: 4,
                                }}>
                                  <Cpu size={10} /> {as_.agent}
                                  <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.2)', marginLeft: 4 }}>
                                    {as_.round === 'synthesis' ? t('disc.synthesis') : `Round ${as_.round}`}
                                  </span>
                                  {!as_.done && <Loader2 size={9} style={{ animation: 'spin 1s linear infinite', marginLeft: 4 }} />}
                                </div>
                                {as_.text ? (
                                  <MarkdownContent content={as_.text} />
                                ) : !as_.done ? (
                                  <div style={{ fontSize: 12, color: 'rgba(255,255,255,0.3)', fontStyle: 'italic' }}>
                                    {t('disc.thinking', as_.agent)}
                                  </div>
                                ) : null}
                              </div>
                            </div>
                          ))}
                        </>
                      );
                    })()}

                    {/* Validation complete banner */}
                    {(() => {
                      if (activeDiscussion.title !== 'Validation audit AI' || !activeDiscussion.project_id) return null;
                      const proj = projects.find(p => p.id === activeDiscussion.project_id);
                      if (!proj || proj.audit_status !== 'Audited') return null;
                      const lastAgentMsg = [...activeDiscussion.messages].reverse().find(m => m.role === 'Agent');
                      const isComplete = lastAgentMsg && lastAgentMsg.content.includes('KRONN:VALIDATION_COMPLETE');
                      if (!isComplete) return null;
                      return (
                        <div style={{ margin: '12px 16px', padding: '12px 16px', borderRadius: 10, background: 'rgba(200,255,0,0.06)', border: '1px solid rgba(200,255,0,0.15)' }}>
                          <p style={{ fontSize: 12, color: 'rgba(200,255,0,0.8)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 6 }}>
                            <ShieldCheck size={14} /> {t('audit.validationComplete')}
                          </p>
                          <button
                            style={{
                              padding: '8px 16px', borderRadius: 8, border: 'none',
                              background: '#c8ff00', color: '#0a0c10', fontWeight: 700,
                              fontSize: 12, fontFamily: 'inherit', cursor: 'pointer',
                              display: 'flex', alignItems: 'center', gap: 6,
                            }}
                            onClick={async () => {
                              await projectsApi.validateAudit(proj.id);
                              refetch();
                              refetchDiscussions();
                            }}
                          >
                            <Check size={12} /> {t('audit.markValid')}
                          </button>
                        </div>
                      );
                    })()}

                    <div ref={chatEndRef} />
                  </div>

                  {/* Input */}
                  <div style={ds.inputBar}>
                    <div style={{ flex: 1, position: 'relative' }}>
                      <input
                        ref={chatInputRef}
                        style={ds.chatInput}
                        placeholder={activeDiscussion && (activeDiscussion.participants?.length ?? 0) > 1 && AGENT_MENTIONS.length > 0
                          ? t('disc.mentionHint', AGENT_MENTIONS.map(m => m.trigger).join(', '))
                          : t('disc.messagePlaceholder')}
                        value={chatInput}
                        onChange={e => {
                          const val = e.target.value;
                          setChatInput(val);
                          // Detect @mention at start
                          const atMatch = val.match(/^@(\w*)$/);
                          if (atMatch) {
                            setMentionQuery(atMatch[1].toLowerCase());
                            setMentionIndex(0);
                          } else {
                            setMentionQuery(null);
                          }
                        }}
                        onKeyDown={e => {
                          if (mentionQuery !== null) {
                            const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
                            if (e.key === 'ArrowDown') { e.preventDefault(); setMentionIndex(i => Math.min(i + 1, filtered.length - 1)); return; }
                            if (e.key === 'ArrowUp') { e.preventDefault(); setMentionIndex(i => Math.max(i - 1, 0)); return; }
                            if ((e.key === 'Tab' || e.key === 'Enter') && filtered.length > 0) {
                              e.preventDefault();
                              setChatInput(filtered[mentionIndex].trigger + ' ');
                              setMentionQuery(null);
                              return;
                            }
                            if (e.key === 'Escape') { setMentionQuery(null); return; }
                          }
                          if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); handleSendMessage(); }
                        }}
                        disabled={sending}
                      />
                      {/* @mention autocomplete dropdown */}
                      {mentionQuery !== null && (() => {
                        const filtered = AGENT_MENTIONS.filter(m => m.trigger.slice(1).startsWith(mentionQuery ?? ''));
                        if (filtered.length === 0) return null;
                        return (
                          <div style={{
                            position: 'absolute', bottom: '100%', left: 0, marginBottom: 4,
                            background: '#1a1d26', border: '1px solid rgba(200,255,0,0.2)',
                            borderRadius: 8, overflow: 'hidden', boxShadow: '0 4px 16px rgba(0,0,0,0.4)',
                            minWidth: 180,
                          }}>
                            {filtered.map((m, i) => (
                              <button
                                key={m.trigger}
                                style={{
                                  display: 'flex', alignItems: 'center', gap: 8,
                                  width: '100%', padding: '8px 12px', border: 'none', cursor: 'pointer',
                                  background: i === mentionIndex ? 'rgba(200,255,0,0.1)' : 'transparent',
                                  color: '#e8eaed', fontFamily: 'inherit', fontSize: 12, textAlign: 'left',
                                }}
                                onMouseDown={e => {
                                  e.preventDefault();
                                  setChatInput(m.trigger + ' ');
                                  setMentionQuery(null);
                                  chatInputRef.current?.focus();
                                }}
                                onMouseEnter={() => setMentionIndex(i)}
                              >
                                <Cpu size={12} style={{ color: '#c8ff00' }} />
                                <span style={{ fontWeight: 600, color: '#c8ff00' }}>{m.trigger}</span>
                                <span style={{ color: 'rgba(255,255,255,0.4)' }}>{m.label}</span>
                              </button>
                            ))}
                          </div>
                        );
                      })()}
                    </div>
                    {/* Debate button */}
                    <div style={{ position: 'relative' }}>
                      <button
                        style={{
                          ...ds.sendBtn,
                          background: showDebatePopover ? 'rgba(139,92,246,0.2)' : 'rgba(139,92,246,0.08)',
                          border: '1px solid rgba(139,92,246,0.3)',
                          color: '#8b5cf6',
                        }}
                        onClick={() => {
                          if (!showDebatePopover) {
                            // Pre-select all installed agents
                            setDebateAgents(installedAgents.map(a => a.agent_type));
                          }
                          setShowDebatePopover(!showDebatePopover);
                        }}
                        disabled={sending}
                        title={t('debate.title')}
                      >
                        <Users size={16} />
                      </button>
                      {showDebatePopover && (
                        <div style={{
                          position: 'absolute', bottom: '100%', right: 0, marginBottom: 8,
                          width: 260, padding: 14, borderRadius: 10,
                          background: '#1a1d26', border: '1px solid rgba(139,92,246,0.2)',
                          boxShadow: '0 8px 32px rgba(0,0,0,0.5)',
                        }}>
                          <div style={{ fontSize: 12, fontWeight: 700, color: '#8b5cf6', marginBottom: 10, display: 'flex', alignItems: 'center', gap: 6 }}>
                            <Users size={12} /> {t('debate.header')}
                          </div>
                          <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 10, lineHeight: 1.4 }}>
                            {t('debate.instructions')}
                          </p>
                          {installedAgents.map(a => {
                            const isPrincipal = a.agent_type === activeDiscussion?.agent;
                            const checked = debateAgents.includes(a.agent_type);
                            return (
                              <label key={a.name} style={{
                                display: 'flex', alignItems: 'center', gap: 8, padding: '6px 0',
                                cursor: isPrincipal ? 'default' : 'pointer', fontSize: 12,
                                color: checked ? '#e8eaed' : 'rgba(255,255,255,0.4)',
                              }}>
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  disabled={isPrincipal}
                                  onChange={() => {
                                    if (isPrincipal) return;
                                    setDebateAgents(prev =>
                                      prev.includes(a.agent_type)
                                        ? prev.filter(t => t !== a.agent_type)
                                        : [...prev, a.agent_type]
                                    );
                                  }}
                                  style={{ accentColor: '#8b5cf6' }}
                                />
                                <Cpu size={11} style={{ color: isPrincipal ? '#c8ff00' : '#8b5cf6' }} />
                                {a.name}
                                {isPrincipal && (
                                  <span style={{ fontSize: 9, color: '#c8ff00', marginLeft: 'auto' }}>{t('debate.main')}</span>
                                )}
                              </label>
                            );
                          })}
                          <button
                            style={{
                              marginTop: 10, width: '100%', padding: '8px 12px', borderRadius: 6,
                              border: 'none', fontFamily: 'inherit', fontSize: 12, fontWeight: 700, cursor: 'pointer',
                              background: debateAgents.length >= 2 ? '#8b5cf6' : 'rgba(255,255,255,0.06)',
                              color: debateAgents.length >= 2 ? '#fff' : 'rgba(255,255,255,0.25)',
                            }}
                            disabled={debateAgents.length < 2}
                            onClick={handleOrchestrate}
                          >
                            {t('debate.launch', debateAgents.length)}
                          </button>
                        </div>
                      )}
                    </div>
                    {sending ? (
                      <button
                        style={{
                          ...ds.sendBtn,
                          background: 'rgba(255,77,106,0.15)',
                          border: '1px solid rgba(255,77,106,0.4)',
                          color: '#ff4d6a',
                        }}
                        onClick={handleStop}
                        title={t('disc.stopThinking')}
                      >
                        <StopCircle size={16} />
                      </button>
                    ) : (
                      <button
                        style={ds.sendBtn}
                        onClick={handleSendMessage}
                        disabled={!chatInput.trim()}
                      >
                        <Send size={16} />
                      </button>
                    )}
                  </div>
                </>
              ) : !showNewDiscussion && (
                <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', flex: 1, color: 'rgba(255,255,255,0.2)' }}>
                  <MessageSquare size={48} style={{ marginBottom: 16, opacity: 0.3 }} />
                  <p style={{ fontSize: 14 }}>{t('disc.selectOrCreate')}</p>
                </div>
              )}
            </div>
          </div>
          );
        })()}

        {/* ════════ CONFIG ════════ */}
        {page === 'settings' && (() => {
          const handleSaveTokens = async () => {
            setTokenSaving(true);
            setTokenSaved(false);
            try {
              await configApi.saveTokens({
                anthropic: tokenInputs.anthropic || null,
                openai: tokenInputs.openai || null,
              });
              setTokenSaved(true);
              setTokenInputs({ anthropic: '', openai: '' });
              refetchTokens();
              setTimeout(() => setTokenSaved(false), 3000);
            } finally {
              setTokenSaving(false);
            }
          };

          const TOKEN_FIELDS = [
            { key: 'anthropic' as const, label: 'Anthropic', hint: 'ANTHROPIC_API_KEY', agents: 'Claude Code' },
            { key: 'openai' as const, label: 'OpenAI', hint: 'OPENAI_API_KEY', agents: 'Codex' },
          ];

          return (
          <div>
            <h1 style={s.h1}>Configuration</h1>
            <p style={{ ...s.meta, marginBottom: 20 }}>{t('config.subtitle')}</p>

            {/* UI Language */}
            <div style={s.card(false)}>
              <div style={{ padding: '16px 20px' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                  <MessageSquare size={14} style={{ color: '#c8ff00' }} />
                  <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.uiLanguage')}</span>
                </div>
                <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
                  {t('config.uiLanguageHint')}
                </p>
                <div style={{ display: 'flex', gap: 8 }}>
                  {UI_LOCALES.map(l => (
                    <button
                      key={l.code}
                      style={{
                        padding: '8px 16px', borderRadius: 6, cursor: 'pointer',
                        fontFamily: 'inherit', fontSize: 12, fontWeight: 600,
                        background: locale === l.code ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.04)',
                        color: locale === l.code ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                        border: locale === l.code ? '1px solid rgba(200,255,0,0.3)' : '1px solid rgba(255,255,255,0.08)',
                      }}
                      onClick={() => setLocale(l.code)}
                    >
                      {l.flag} {l.label}
                    </button>
                  ))}
                </div>
              </div>
            </div>

            {/* Output Language */}
            <div style={s.card(false)}>
              <div style={{ padding: '16px 20px' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                  <MessageSquare size={14} style={{ color: '#c8ff00' }} />
                  <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.outputLanguage')}</span>
                </div>
                <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
                  {t('config.outputLanguageHint')}
                </p>
                <div style={{ display: 'flex', gap: 8 }}>
                  {LANGUAGES.map(l => (
                    <button
                      key={l.code}
                      style={{
                        padding: '8px 16px', borderRadius: 6, cursor: 'pointer',
                        fontFamily: 'inherit', fontSize: 12, fontWeight: 600,
                        background: (configLanguage ?? 'fr') === l.code ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.04)',
                        color: (configLanguage ?? 'fr') === l.code ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                        border: (configLanguage ?? 'fr') === l.code ? '1px solid rgba(200,255,0,0.3)' : '1px solid rgba(255,255,255,0.08)',
                      }}
                      onClick={async () => {
                        await configApi.saveLanguage(l.code);
                        refetchLanguage();
                      }}
                    >
                      {l.flag} {l.label}
                    </button>
                  ))}
                </div>
              </div>
            </div>

            {/* Agents */}
            <div style={s.card(false)}>
              <div style={{ padding: '16px 20px' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
                  <Cpu size={14} style={{ color: '#c8ff00' }} />
                  <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.agents')}</span>
                  <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
                    {agents.filter(a => a.installed).length}/{agents.length} {agents.filter(a => a.installed).length > 1 ? t('config.installedPlural') : t('config.installed')}
                  </span>
                  <button style={s.iconBtn} onClick={() => refetchAgents()} title={t('config.refresh')}>
                    <RefreshCw size={12} />
                  </button>
                </div>

                {agents.map(agent => {
                  const permFlag: Record<string, { flag: string; descKey: string }> = {
                    ClaudeCode: { flag: '--dangerously-skip-permissions', descKey: 'config.fullAccess' },
                    Codex: { flag: '--full-auto', descKey: 'config.autoApply' },
                  };
                  const perm = permFlag[agent.agent_type];
                  const isFullAccess = agent.agent_type === 'ClaudeCode'
                    ? agentAccess?.claude_code?.full_access ?? false
                    : agent.agent_type === 'Codex'
                      ? agentAccess?.codex?.full_access ?? false
                      : false;

                  return (
                  <div key={agent.name} style={{ padding: '10px 0', borderTop: '1px solid rgba(255,255,255,0.05)' }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                      <div style={s.dot(agent.installed && agent.enabled)} />
                      <div style={{ flex: 1 }}>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                          <span style={{ fontWeight: 600, fontSize: 12 }}>{agent.name}</span>
                          <span style={s.originBadge}>{agent.origin}</span>
                          {agent.version && <code style={{ ...s.code, fontSize: 10 }}>v{agent.version}</code>}
                          {agent.latest_version && agent.latest_version !== agent.version && (
                            <span style={s.updateBadge}>&#x2B06; {agent.latest_version}</span>
                          )}
                        </div>
                        {!agent.installed && (
                          <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.25)', marginTop: 2 }}>
                            <code style={s.code}>{agent.install_command}</code>
                          </div>
                        )}
                      </div>
                      {agent.installed ? (
                        <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                          <button
                            style={{
                              ...s.iconBtn,
                              fontSize: 10,
                              padding: '2px 8px',
                              borderRadius: 4,
                              background: agent.enabled ? 'rgba(200,255,0,0.1)' : 'rgba(255,100,0,0.1)',
                              border: agent.enabled ? '1px solid rgba(200,255,0,0.2)' : '1px solid rgba(255,100,0,0.2)',
                              color: agent.enabled ? '#c8ff00' : '#ff8c00',
                            }}
                            title={agent.enabled ? t('config.toggleDisable') : t('config.toggleEnable')}
                            onClick={async () => {
                              try {
                                await agentsApi.toggle(agent.agent_type);
                              } catch { /* ignore */ }
                              refetchAgents();
                            }}
                            disabled={installing !== null}
                          >
                            {agent.enabled ? t('config.enabled') : t('config.disabled')}
                          </button>
                          <button
                            style={{ ...s.iconBtn, color: 'rgba(255,255,255,0.2)' }}
                            title={t('config.uninstall')}
                            onClick={async () => {
                              if (!confirm(t('config.uninstallConfirm', agent.name))) return;
                              setInstalling(agent.name);
                              try {
                                await agentsApi.uninstall(agent.agent_type);
                              } catch { /* ignore */ }
                              refetchAgents();
                              setInstalling(null);
                            }}
                            disabled={installing !== null}
                          >
                            <Trash2 size={11} />
                          </button>
                        </div>
                      ) : (
                        <button
                          style={{ ...s.installBtn, padding: '4px 10px', fontSize: 11 }}
                          onClick={() => handleInstallAgent(agent)}
                          disabled={installing !== null}
                        >
                          {installing === agent.name ? (
                            <><Loader2 size={10} style={{ animation: 'spin 1s linear infinite' }} /> ...</>
                          ) : (
                            <><Download size={10} /> Installer</>
                          )}
                        </button>
                      )}
                    </div>
                    {perm && agent.installed && (
                      <div style={{ marginLeft: 22, marginTop: 8, padding: '8px 12px', borderRadius: 6, background: 'rgba(255,255,255,0.02)', border: '1px solid rgba(255,255,255,0.05)' }}>
                        <div
                          style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                          onClick={async () => {
                            await configApi.setAgentAccess({ agent: agent.agent_type, full_access: !isFullAccess });
                            refetchAgentAccess();
                          }}
                        >
                          <div style={{
                            width: 30, height: 16, borderRadius: 8, position: 'relative' as const, transition: 'background 0.2s',
                            background: isFullAccess ? 'rgba(200,255,0,0.3)' : 'rgba(255,255,255,0.1)',
                            border: isFullAccess ? '1px solid rgba(200,255,0,0.4)' : '1px solid rgba(255,255,255,0.15)',
                          }}>
                            <div style={{
                              width: 12, height: 12, borderRadius: '50%', position: 'absolute' as const, top: 1, transition: 'left 0.2s',
                              left: isFullAccess ? 16 : 1,
                              background: isFullAccess ? '#c8ff00' : 'rgba(255,255,255,0.3)',
                            }} />
                          </div>
                          <code style={{ fontSize: 10, color: isFullAccess ? '#c8ff00' : 'rgba(255,255,255,0.4)' }}>{perm.flag}</code>
                        </div>
                        <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.3)', marginTop: 4, marginLeft: 38 }}>
                          {t(perm.descKey)}
                        </p>
                      </div>
                    )}
                  </div>
                  );
                })}
              </div>
            </div>

            {/* API Keys */}
            <div id="api-keys" style={s.card(false)}>
              <div style={{ padding: '16px 20px' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                  <Key size={14} style={{ color: '#c8ff00' }} />
                  <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.apiKeys')}</span>
                </div>
                <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 6 }}>
                  {t('config.apiKeysHint1')}
                </p>
                <p style={{ color: 'rgba(255,255,255,0.25)', fontSize: 11, marginBottom: 16 }}>
                  {t('config.apiKeysHint2')}
                </p>

                {TOKEN_FIELDS.map(({ key, label, hint, agents: agentNames }) => {
                  const current = tokenConfig?.[key];
                  const isSet = !!current;
                  return (
                    <div key={key} style={{ marginBottom: 14 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
                        <label style={{ fontSize: 12, fontWeight: 600, color: 'rgba(255,255,255,0.6)' }}>{label}</label>
                        {isSet ? (
                          <span style={{ fontSize: 10, padding: '1px 6px', borderRadius: 4, background: 'rgba(52,211,153,0.1)', color: 'rgba(52,211,153,0.7)' }}>
                            <Check size={9} /> override: {current}
                          </span>
                        ) : (
                          <span style={{ fontSize: 10, padding: '1px 6px', borderRadius: 4, background: 'rgba(255,255,255,0.04)', color: 'rgba(255,255,255,0.3)' }}>
                            {t('config.localAuth')}
                          </span>
                        )}
                        <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.2)', marginLeft: 'auto' }}>{agentNames}</span>
                      </div>
                      <input
                        type="password"
                        style={s.input}
                        placeholder={hint}
                        value={tokenInputs[key]}
                        onChange={e => setTokenInputs(prev => ({ ...prev, [key]: e.target.value }))}
                      />
                    </div>
                  );
                })}

                <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 8 }}>
                  <button
                    style={{ ...s.scanBtn, opacity: (tokenInputs.anthropic || tokenInputs.openai) ? 1 : 0.4 }}
                    onClick={handleSaveTokens}
                    disabled={tokenSaving || (!tokenInputs.anthropic && !tokenInputs.openai)}
                  >
                    <Save size={12} /> {tokenSaving ? t('config.save') + '...' : t('config.save')}
                  </button>
                  {tokenSaved && (
                    <span style={{ fontSize: 12, color: '#34d399' }}><Check size={12} /> {t('config.saved')}</span>
                  )}
                </div>
              </div>
            </div>

            {/* Database */}
            <div style={s.card(false)}>
              <div style={{ padding: '16px 20px' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 12 }}>
                  <HardDrive size={14} style={{ color: '#c8ff00' }} />
                  <span style={{ fontWeight: 600, fontSize: 14 }}>{t('config.database')}</span>
                  {dbInfo && (
                    <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.3)', marginLeft: 'auto' }}>
                      {dbInfo.size_bytes < 1024 * 1024
                        ? `${(dbInfo.size_bytes / 1024).toFixed(1)} Ko`
                        : `${(dbInfo.size_bytes / (1024 * 1024)).toFixed(1)} Mo`}
                    </span>
                  )}
                </div>

                {dbInfo && (
                  <div style={{ display: 'flex', gap: 16, marginBottom: 16 }}>
                    {[
                      { label: t('config.dbProjects'), value: dbInfo.project_count },
                      { label: t('config.dbDiscussions'), value: dbInfo.discussion_count },
                      { label: t('config.dbMessages'), value: dbInfo.message_count },
                      { label: t('config.dbMcps'), value: dbInfo.mcp_count },
                      { label: t('config.dbTasks'), value: dbInfo.task_count },
                    ].map(({ label, value }) => (
                      <div key={label} style={{ textAlign: 'center' }}>
                        <div style={{ fontSize: 18, fontWeight: 700, color: '#c8ff00' }}>{value}</div>
                        <div style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)', marginTop: 2 }}>{label}</div>
                      </div>
                    ))}
                  </div>
                )}

                <div style={{ display: 'flex', gap: 8 }}>
                  <button
                    style={s.scanBtn}
                    onClick={async () => {
                      const data = await configApi.exportData();
                      if (!data) return;
                      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
                      const url = URL.createObjectURL(blob);
                      const a = document.createElement('a');
                      a.href = url;
                      a.download = `kronn-export-${new Date().toISOString().slice(0, 10)}.json`;
                      a.click();
                      URL.revokeObjectURL(url);
                    }}
                  >
                    <Download size={12} /> {t('config.export')}
                  </button>
                  <button
                    style={s.scanBtn}
                    onClick={() => {
                      const input = document.createElement('input');
                      input.type = 'file';
                      input.accept = '.json';
                      input.onchange = async () => {
                        const file = input.files?.[0];
                        if (!file) return;
                        try {
                          const text = await file.text();
                          const data = JSON.parse(text);
                          if (!data.version || !data.projects || !data.discussions) {
                            alert(t('config.importInvalid'));
                            return;
                          }
                          if (!confirm(t('config.importConfirm'))) return;
                          await configApi.importData(data);
                          refetch();
                          refetchDiscussions();
                          refetchDbInfo();
                        } catch {
                          alert(t('config.importError'));
                        }
                      };
                      input.click();
                    }}
                  >
                    <Upload size={12} /> {t('config.import')}
                  </button>
                </div>
              </div>
            </div>

            {/* General */}
            <div style={{ ...s.card(false), marginTop: 16 }}>
              <div style={{ padding: '16px 20px' }}>
                <p style={{ color: 'rgba(255,255,255,0.4)', fontSize: 13, marginBottom: 16 }}>
                  Fichier de config : <code style={s.code}>~/.config/kronn/config.toml</code>
                </p>
                <div style={{ borderTop: '1px solid rgba(255,255,255,0.05)', paddingTop: 16 }}>
                  <p style={{ color: 'rgba(255,255,255,0.35)', fontSize: 12, marginBottom: 12 }}>
                    {t('config.resetHint')}
                  </p>
                  <button style={s.dangerBtn} onClick={onReset}>
                    <Trash2 size={12} /> {t('config.reset')}
                  </button>
                </div>
              </div>
            </div>
          </div>
          );
        })()}
      </main>
    </div>
  );
}

// ─── Styles ─────────────────────────────────────────────────────────────────

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

// ─── Discussion styles ──────────────────────────────────────────────────────

const mdStyles: Record<string, React.CSSProperties> = {
  p: { margin: '4px 0' },
  h1: { fontSize: 18, fontWeight: 700, margin: '12px 0 6px', color: '#e8eaed' },
  h2: { fontSize: 16, fontWeight: 700, margin: '10px 0 4px', color: '#e8eaed' },
  h3: { fontSize: 14, fontWeight: 600, margin: '8px 0 4px', color: '#e8eaed' },
  ul: { margin: '4px 0', paddingLeft: 20 },
  ol: { margin: '4px 0', paddingLeft: 20 },
  li: { margin: '2px 0' },
  code: { background: 'rgba(255,255,255,0.08)', padding: '1px 5px', borderRadius: 4, fontSize: 12, fontFamily: 'monospace' },
  pre: { background: 'rgba(0,0,0,0.3)', padding: '10px 12px', borderRadius: 8, overflowX: 'auto', margin: '6px 0', border: '1px solid rgba(255,255,255,0.06)' },
  preCode: { background: 'none', padding: 0, fontSize: 12, fontFamily: 'monospace', color: '#c8ff00' },
  table: { borderCollapse: 'collapse' as const, width: '100%', margin: '8px 0', fontSize: 12 },
  th: { border: '1px solid rgba(255,255,255,0.12)', padding: '6px 10px', background: 'rgba(255,255,255,0.05)', fontWeight: 600, textAlign: 'left' as const },
  td: { border: '1px solid rgba(255,255,255,0.08)', padding: '5px 10px' },
  blockquote: { borderLeft: '3px solid rgba(200,255,0,0.3)', margin: '6px 0', paddingLeft: 12, color: 'rgba(255,255,255,0.6)' },
  hr: { border: 'none', borderTop: '1px solid rgba(255,255,255,0.1)', margin: '10px 0' },
  a: { color: '#c8ff00', textDecoration: 'underline' },
  strong: { fontWeight: 700, color: '#f0f0f0' },
};

const MarkdownContent = ({ content }: { content: string }) => (
  <div style={{ fontSize: 13, lineHeight: 1.55 }}>
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        p: ({ children }) => <p style={mdStyles.p}>{children}</p>,
        h1: ({ children }) => <h1 style={mdStyles.h1}>{children}</h1>,
        h2: ({ children }) => <h2 style={mdStyles.h2}>{children}</h2>,
        h3: ({ children }) => <h3 style={mdStyles.h3}>{children}</h3>,
        ul: ({ children }) => <ul style={mdStyles.ul}>{children}</ul>,
        ol: ({ children }) => <ol style={mdStyles.ol}>{children}</ol>,
        li: ({ children }) => <li style={mdStyles.li}>{children}</li>,
        code: ({ className, children }) => {
          const isBlock = className?.includes('language-');
          return isBlock
            ? <code style={mdStyles.preCode}>{children}</code>
            : <code style={mdStyles.code}>{children}</code>;
        },
        pre: ({ children }) => <pre style={mdStyles.pre}>{children}</pre>,
        table: ({ children }) => <table style={mdStyles.table}>{children}</table>,
        th: ({ children }) => <th style={mdStyles.th}>{children}</th>,
        td: ({ children }) => <td style={mdStyles.td}>{children}</td>,
        blockquote: ({ children }) => <blockquote style={mdStyles.blockquote}>{children}</blockquote>,
        hr: () => <hr style={mdStyles.hr} />,
        a: ({ href, children }) => <a href={href} style={mdStyles.a} target="_blank" rel="noopener noreferrer">{children}</a>,
        strong: ({ children }) => <strong style={mdStyles.strong}>{children}</strong>,
      }}
    >
      {content}
    </ReactMarkdown>
  </div>
);

const ds = {
  sidebar: { width: 280, borderRight: '1px solid rgba(255,255,255,0.07)', background: '#0e1117', display: 'flex', flexDirection: 'column' as const, flexShrink: 0 },
  sidebarHeader: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '14px 14px 10px', borderBottom: '1px solid rgba(255,255,255,0.06)' } as const,
  sidebarList: { flex: 1, overflowY: 'auto' as const, padding: '8px 0' },
  projectGroup: {
    fontSize: 10, fontWeight: 700, textTransform: 'uppercase' as const, letterSpacing: '0.06em',
    color: 'rgba(200,255,0,0.5)', padding: '14px 14px 6px',
    marginTop: 4, borderTop: '1px solid rgba(255,255,255,0.05)',
    display: 'flex', alignItems: 'center', gap: 6,
  },
  discItem: (active: boolean) => ({
    display: 'flex', alignItems: 'center', gap: 8, width: '100%', padding: '8px 14px 8px 22px', border: 'none',
    background: active ? 'rgba(200,255,0,0.06)' : 'transparent',
    borderLeft: active ? '2px solid #c8ff00' : '2px solid transparent',
    color: active ? '#e8eaed' : 'rgba(255,255,255,0.5)',
    cursor: 'pointer', textAlign: 'left' as const, fontFamily: 'inherit',
  }),
  chatArea: { flex: 1, display: 'flex', flexDirection: 'column' as const, minWidth: 0, background: '#0a0c10' },
  chatHeader: { display: 'flex', alignItems: 'center', gap: 12, padding: '14px 20px', borderBottom: '1px solid rgba(255,255,255,0.07)', background: '#12151c', flexShrink: 0 } as const,
  messages: { flex: 1, overflowY: 'auto' as const, padding: '20px 20px 10px' },
  msgRow: (isUser: boolean) => ({ display: 'flex', justifyContent: isUser ? 'flex-end' : 'flex-start', marginBottom: 12 }),
  msgBubble: (isUser: boolean) => ({
    maxWidth: '70%', padding: '10px 14px', borderRadius: 12,
    background: isUser ? 'rgba(200,255,0,0.08)' : 'rgba(255,255,255,0.04)',
    border: `1px solid ${isUser ? 'rgba(200,255,0,0.15)' : 'rgba(255,255,255,0.07)'}`,
    color: '#e8eaed',
  }),
  msgAgent: { display: 'flex', alignItems: 'center', gap: 4, fontSize: 10, fontWeight: 600, color: 'rgba(139,92,246,0.7)', marginBottom: 4 } as const,
  msgTime: { fontSize: 10, color: 'rgba(255,255,255,0.2)', marginTop: 4, textAlign: 'right' as const },
  inputBar: { display: 'flex', gap: 8, padding: '12px 20px', borderTop: '1px solid rgba(255,255,255,0.07)', background: '#12151c', flexShrink: 0 } as const,
  chatInput: { width: '100%', padding: '10px 14px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none' } as const,
  sendBtn: { padding: '10px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.2)', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', cursor: 'pointer', display: 'flex', alignItems: 'center', justifyContent: 'center' } as const,
  newDiscOverlay: { display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 1 } as const,
  newDiscCard: { width: 420, padding: 24, borderRadius: 12, background: '#12151c', border: '1px solid rgba(255,255,255,0.1)' } as const,
  label: { display: 'block', fontSize: 11, fontWeight: 600, color: 'rgba(255,255,255,0.5)', marginBottom: 4, marginTop: 8 } as const,
  select: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', outline: 'none' } as const,
  selectStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    cursor: 'pointer', appearance: 'none' as const,
    backgroundImage: `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%23888' stroke-width='2'%3E%3Cpolyline points='6 9 12 15 18 9'%3E%3C/polyline%3E%3C/svg%3E")`,
    backgroundRepeat: 'no-repeat', backgroundPosition: 'right 10px center',
    paddingRight: 32,
  } as const,
  inputStyled: {
    width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    boxSizing: 'border-box' as const,
  } as const,
  textareaStyled: {
    width: '100%', padding: '10px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
    borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit', outline: 'none',
    resize: 'vertical' as const, boxSizing: 'border-box' as const, lineHeight: 1.5,
  } as const,
  textarea: { width: '100%', padding: '10px 12px', background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', outline: 'none', resize: 'vertical' as const } as const,
};
