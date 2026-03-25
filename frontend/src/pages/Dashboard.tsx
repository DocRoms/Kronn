import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { projects as projectsApi, mcps as mcpsApi, agents as agentsApi, discussions as discussionsApi, workflows as workflowsApi, config as configApi, skills as skillsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useToast } from '../hooks/useToast';
import type { Project, AgentDetection, AgentType, RemoteRepo, RepoSource, DriftCheckResponse } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { getProjectGroup, isHiddenPath, isUsable, isValidationDisc } from '../lib/constants';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { McpPage } from './McpPage';
import { WorkflowsPage } from './WorkflowsPage';
import { SettingsPage } from './SettingsPage';
import { DiscussionsPage } from './DiscussionsPage';
import { AiDocViewer } from '../components/AiDocViewer';
import { ProjectSkills } from '../components/ProjectSkills';
import {
  Folder, Server, ChevronRight, ChevronDown, Cpu, Workflow,
  Plus, Trash2, Search, Zap, Settings, Eye,
  Loader2,
  MessageSquare, X, AlertTriangle,
  Play, FileCode, ShieldCheck, StopCircle, BookOpen, Rocket, Check, RefreshCw,
} from 'lucide-react';

type Page = 'projects' | 'mcps' | 'workflows' | 'discussions' | 'settings';

interface DashboardProps {
  onReset: () => void;
}


const isAiReady = (p: Project) => p.audit_status !== 'NoTemplate';

const STATUS_COLORS: Record<string, string> = {
  Pending: '#ffc800', Running: '#00d4ff', Success: '#34d399',
  Failed: '#ff4d6a', Cancelled: 'rgba(255,255,255,0.3)', WaitingApproval: '#c8ff00',
};

/** Agents that can run audits/briefings (need filesystem access + CLI mode). Excludes Vibe (API-only). */
const canAudit = (a: AgentDetection) => isUsable(a) && a.agent_type !== 'Vibe';



// Sort score for project readiness
export function Dashboard({ onReset }: DashboardProps) {
  const { t } = useT();
  const isMobile = useIsMobile();
  const { toast, ToastContainer } = useToast();
  const [page, setPage] = useState<Page>('projects');
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  // Cross-page prefill for discussion creation (e.g. "validate audit" from Projects)
  const [discPrefill, setDiscPrefill] = useState<{ projectId: string; title: string; prompt: string; locked?: boolean } | null>(null);
  // Unseen message tracking (persisted in localStorage, computed in Dashboard)
  const [lastSeenMsgCount, setLastSeenMsgCount] = useState<Record<string, number>>(() => {
    try { return JSON.parse(localStorage.getItem('kronn:lastSeenMsgCount') ?? '{}'); } catch { return {}; }
  });
  const [activeDiscussionId, setActiveDiscussionId] = useState<string | null>(null);
  // Auto-run agent on a discussion (after full audit creates validation discussion)
  const [autoRunDiscussionId, setAutoRunDiscussionId] = useState<string | null>(null);
  // Open a specific discussion without triggering agent (e.g. Resume Validation button)
  const [openDiscussionId, setOpenDiscussionId] = useState<string | null>(null);
  // AI audit state
  const [auditAgentChoice, setAuditAgentChoice] = useState<Record<string, AgentType>>({});
  // Collapsible sections per project: tracks which section is open (only one at a time)
  const [openSections, setOpenSections] = useState<Record<string, string>>({});
  const toggleSection = useCallback((projectId: string, section: string) => {
    setOpenSections(prev => ({
      ...prev,
      [projectId]: prev[projectId] === section ? '' : section,
    }));
  }, []);
  const defaultSection = useCallback((auditStatus: string) => {
    // Before audit completes: show AI Context to encourage launching audit
    // After audit (Audited/Validated): show Discussions (auto-created post-audit)
    return (auditStatus === 'Bootstrapped' || auditStatus === 'Audited' || auditStatus === 'Validated') ? 'discussions' : 'aiContext';
  }, []);
  const isSectionOpen = useCallback((projectId: string, section: string, auditStatus: string) => {
    if (openSections[projectId] === undefined) return section === defaultSection(auditStatus);
    return openSections[projectId] === section;
  }, [openSections, defaultSection]);
  const [auditState, setAuditState] = useState<Record<string, {
    active: boolean;
    step: number;
    totalSteps: number;
    currentFile: string;
  }>>({});
  const auditAbortControllers = useRef<Record<string, AbortController>>({});

  // ─── Drift detection state ──────────
  const [driftByProject, setDriftByProject] = useState<Record<string, DriftCheckResponse>>({});

  // ─── Lifted discussion streaming state (survives page changes) ──────────
  const [sendingMap, setSendingMap] = useState<Record<string, boolean>>({});
  const [sendingStartMap, setSendingStartMap] = useState<Record<string, number>>({});
  const [streamingMap, setStreamingMap] = useState<Record<string, string>>({});
  const abortControllers = useRef<Record<string, AbortController>>({});

  const cleanupStream = useCallback((discId: string) => {
    setSendingMap(prev => ({ ...prev, [discId]: false }));
    setSendingStartMap(prev => { const n = { ...prev }; delete n[discId]; return n; });
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
  const { data: skillList, refetch: refetchSkills } = useApi(() => skillsApi.list(), []);

  // Poll discussions for notifications — faster when on discussions page, slower otherwise
  useEffect(() => {
    const pollInterval = page === 'discussions' ? 15000 : 30000;
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

  // ─── Drift detection fetch ──────────
  useEffect(() => {
    for (const proj of projects) {
      if (proj.audit_status === 'Audited' || proj.audit_status === 'Validated') {
        projectsApi.checkDrift(proj.id).then(drift => {
          if (drift) setDriftByProject(prev => ({ ...prev, [proj.id]: drift }));
        }).catch(() => {});
      }
    }
  }, [projects]);

  const mcpRegistry = registry ?? [];
  const mcpOverview = mcpOverviewData ?? { servers: [], configs: [], customized_contexts: [], incompatibilities: [] };
  const agents = agentList ?? [];
  const allDiscussions = discussionList ?? [];
  const allSkills = skillList ?? [];

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

  // O(N) group-by map — avoids O(N×M) inline filter per project in the render loop
  const discussionsByProject = useMemo(() => {
    const map: Record<string, typeof allDiscussions> = {};
    for (const d of allDiscussions) {
      if (d.project_id) {
        (map[d.project_id] ??= []).push(d);
      }
    }
    return map;
  }, [allDiscussions]);

  useEffect(() => {
    document.title = totalUnseen > 0 ? `(${totalUnseen}) Kronn` : 'Kronn';
  }, [totalUnseen]);

  // Stable callback for prefill consumed
  const handlePrefillConsumed = useCallback(() => setDiscPrefill(null), []);
  const handleAutoRunConsumed = useCallback(() => setAutoRunDiscussionId(null), []);
  const handleOpenDiscConsumed = useCallback(() => setOpenDiscussionId(null), []);

  const handleDeleteProject = async (id: string, hard: boolean) => {
    await projectsApi.delete(id, hard);
    setDeleteConfirmId(null);
    setDeleteConfirmInput('');
    refetch();
  };

  const handleCancelAudit = async (projectId: string) => {
    // Abort the SSE stream
    auditAbortControllers.current[projectId]?.abort();
    delete auditAbortControllers.current[projectId];

    // Call backend to kill process + clean files
    try {
      await projectsApi.cancelAudit(projectId);
      toast(t('audit.cancelled'), 'success');
    } catch (e) {
      console.error('Cancel audit failed:', e);
    }

    setAuditState(prev => ({
      ...prev,
      [projectId]: { ...prev[projectId], active: false }
    }));
    refetch();
    refetchDiscussions();
  };

  const handleFullAudit = async (projectId: string) => {
    const controller = new AbortController();
    auditAbortControllers.current[projectId] = controller;

    setAuditState(prev => ({
      ...prev,
      [projectId]: { active: true, step: 0, totalSteps: 10, currentFile: t('audit.templateStep') }
    }));
    try {
      const auditAgent = auditAgentChoice[projectId] ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
      await projectsApi.fullAuditStream(projectId, { agent: auditAgent }, {
        onTemplateInstalled: () => { /* template phase done */ },
        onStepStart: (step, total, file) => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { active: true, step, totalSteps: total, currentFile: file }
          }));
        },
        onChunk: () => {},
        onStepDone: () => {},
        onValidationCreated: () => {},
        onDone: (discussionId) => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { ...prev[projectId], active: false }
          }));
          refetch();
          refetchDiscussions();
          if (discussionId) {
            toast(t('audit.fullAuditDone'), 'success');
            // Navigate to discussions and auto-trigger agent response
            setAutoRunDiscussionId(discussionId);
            setPage('discussions');
          }
        },
        onError: (error) => {
          console.error('Full audit error:', error);
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return; // cancelled by user
      console.error('Full audit failed:', e);
      setAuditState(prev => ({
        ...prev,
        [projectId]: { ...prev[projectId], active: false }
      }));
    } finally {
      delete auditAbortControllers.current[projectId];
    }
  };

  const startPartialAudit = async (projectId: string, drift: DriftCheckResponse) => {
    const steps = drift.stale_sections.map(s => s.audit_step);
    const controller = new AbortController();
    auditAbortControllers.current[projectId] = controller;

    const auditAgent = auditAgentChoice[projectId] ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';

    setAuditState(prev => ({
      ...prev,
      [projectId]: { active: true, step: 0, totalSteps: steps.length, currentFile: '' }
    }));

    try {
      await projectsApi.partialAuditStream(projectId, { agent: auditAgent, steps }, {
        onStepStart: (step, total, file) => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { active: true, step, totalSteps: total, currentFile: file }
          }));
        },
        onChunk: () => {},
        onStepDone: () => {},
        onDone: () => {
          setAuditState(prev => ({
            ...prev,
            [projectId]: { ...prev[projectId], active: false }
          }));
          refetch();
          // Refetch drift to update the badge
          projectsApi.checkDrift(projectId).then(d => {
            if (d) setDriftByProject(prev => ({ ...prev, [projectId]: d }));
          }).catch(() => {});
          toast(t('audit.updateStale', String(steps.length)), 'success');
        },
        onError: (error) => {
          console.error('Partial audit error:', error);
        },
      }, controller.signal);
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return;
      console.error('Partial audit failed:', e);
      setAuditState(prev => ({
        ...prev,
        [projectId]: { ...prev[projectId], active: false }
      }));
    } finally {
      delete auditAbortControllers.current[projectId];
    }
  };

  // Bootstrap new project state
  const [showBootstrap, setShowBootstrap] = useState(false);
  const [bootstrapName, setBootstrapName] = useState('');
  const [bootstrapDesc, setBootstrapDesc] = useState('');
  const [bootstrapLoading, setBootstrapLoading] = useState(false);
  const [bootstrapMcpIds, setBootstrapMcpIds] = useState<string[]>([]);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [deleteConfirmInput, setDeleteConfirmInput] = useState('');
  const [newProjectMode, setNewProjectMode] = useState<'bootstrap' | 'clone'>('bootstrap');
  const [cloneUrl, setCloneUrl] = useState('');
  const [cloneName, setCloneName] = useState('');
  const [cloneLoading, setCloneLoading] = useState(false);
  const [discoveredRepos, setDiscoveredRepos] = useState<RemoteRepo[]>([]);
  const [discoverLoading, setDiscoverLoading] = useState(false);
  const [discoverSources, setDiscoverSources] = useState<string[]>([]);
  const [discoverError, setDiscoverError] = useState('');
  const [availableSources, setAvailableSources] = useState<RepoSource[]>([]);
  const [selectedSourceIds, setSelectedSourceIds] = useState<string[]>([]);
  const [repoSearch, setRepoSearch] = useState('');

  const handleBootstrap = async () => {
    if (!bootstrapName.trim() || !bootstrapDesc.trim()) return;
    const agent = agents.find(a => canAudit(a))?.agent_type;
    if (!agent) { toast('No usable agent found', 'error'); return; }
    setBootstrapLoading(true);
    try {
      const res = await projectsApi.bootstrap({ name: bootstrapName.trim(), description: bootstrapDesc.trim(), agent, mcp_config_ids: bootstrapMcpIds });
      setShowBootstrap(false);
      setBootstrapName('');
      setBootstrapDesc('');
      setBootstrapMcpIds([]);
      await refetch();
      // Navigate to discussions with auto-run on the bootstrap discussion
      setAutoRunDiscussionId(res.discussion_id);
      setPage('discussions');
      toast(`Projet "${bootstrapName}" cree`, 'success');
    } catch (e) {
      toast(`Erreur: ${e}`, 'error');
    } finally {
      setBootstrapLoading(false);
    }
  };

  const handleClone = async () => {
    if (!cloneUrl.trim()) return;
    const agent = agents.find(a => canAudit(a))?.agent_type;
    if (!agent) { toast('No usable agent found', 'error'); return; }
    setCloneLoading(true);
    try {
      await projectsApi.clone({ url: cloneUrl.trim(), name: cloneName.trim() || null, agent });
      setShowBootstrap(false);
      setCloneUrl('');
      setCloneName('');
      await refetch();
      toast(t('projects.clone.success'), 'success');
    } catch (e) {
      toast(`Error: ${e}`, 'error');
    } finally {
      setCloneLoading(false);
    }
  };

  const handleDiscoverRepos = async (sourceIds?: string[]) => {
    setDiscoverLoading(true);
    setDiscoverError('');
    try {
      const res = await projectsApi.discoverRepos({ source_ids: sourceIds ?? selectedSourceIds });
      setDiscoveredRepos(res.repos);
      setDiscoverSources(res.sources);
      setAvailableSources(res.available_sources);
      // On first call, auto-select all sources
      if (selectedSourceIds.length === 0 && res.available_sources.length > 0) {
        setSelectedSourceIds(res.available_sources.map(s => s.id));
      }
    } catch (e) {
      setDiscoverError(String(e));
    } finally {
      setDiscoverLoading(false);
    }
  };

  const toggleSource = (id: string) => {
    const next = selectedSourceIds.includes(id)
      ? selectedSourceIds.filter(x => x !== id)
      : [...selectedSourceIds, id];
    setSelectedSourceIds(next);
    if (next.length > 0) {
      handleDiscoverRepos(next);
    }
  };

  const handleCloneDiscovered = async (repo: RemoteRepo) => {
    const agent = agents.find(a => isUsable(a))?.agent_type;
    if (!agent) { toast('No usable agent found', 'error'); return; }
    setCloneLoading(true);
    try {
      await projectsApi.clone({ url: repo.clone_url, name: repo.name, agent });
      // Mark as cloned in local state
      setDiscoveredRepos(prev => prev.map(r => r.full_name === repo.full_name ? { ...r, already_cloned: true } : r));
      await refetch();
      toast(t('projects.clone.success'), 'success');
    } catch (e) {
      toast(`Error: ${e}`, 'error');
    } finally {
      setCloneLoading(false);
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
      <style>{`@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}`}</style>
      {/* Nav */}
      <nav style={s.nav}>
        <div style={{ ...s.navBrand, ...(isMobile ? { marginRight: 8 } : {}) }}>
          <Zap size={18} style={{ color: '#c8ff00' }} />
          {!isMobile && <span style={s.navTitle}>Kronn</span>}
          {isMobile && <span style={{ ...s.navTitle, fontSize: 12 }}>K</span>}
        </div>
        <div style={isMobile ? { display: 'flex', gap: 4, overflowX: 'auto', WebkitOverflowScrolling: 'touch', scrollbarWidth: 'none', flex: 1 } as React.CSSProperties : { display: 'contents' }}>
        {([
          ['projects', Folder, t('nav.projects')],
          ['discussions', MessageSquare, t('nav.discussions')],
          ['mcps', Server, t('nav.mcps')],
          ['workflows', Workflow, t('nav.workflows')],
          ['settings', Settings, t('nav.config')],
        ] as [string, typeof Folder, string][]).map(([id, Icon, label]) => (
          <button key={id} style={{ ...s.navBtn(page === id), position: 'relative', ...(isMobile ? { padding: '6px 8px', fontSize: 10, whiteSpace: 'nowrap', gap: 3 } : {}) } as React.CSSProperties} onClick={() => setPage(id as Page)} title={label}>
            {id === 'workflows' && runningWorkflows > 0
              ? <Loader2 size={isMobile ? 16 : 14} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />
              : <Icon size={isMobile ? 16 : 14} />
            }
            {!isMobile && <>{' '}{label}</>}
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
        </div>
        <div style={{ flex: isMobile ? 0 : 1 }} />
        <button style={s.scanBtn} onClick={() => setShowBootstrap(true)} title={t('projects.bootstrap')}>
          <Plus size={14} /> {!isMobile && t('projects.bootstrap')}
        </button>
        <button style={s.scanBtn} onClick={handleScan} title={t('nav.scan')}>
          <Search size={14} /> {!isMobile && t('nav.scan')}
        </button>
      </nav>

      {/* Bootstrap modal */}
      {showBootstrap && (
        <div style={{
          position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.7)', zIndex: 1000,
          display: 'flex', alignItems: 'center', justifyContent: 'center',
        }} onClick={() => !bootstrapLoading && !cloneLoading && setShowBootstrap(false)}>
          <div
            style={{
              background: '#161b22', border: '1px solid rgba(200,255,0,0.15)', borderRadius: 12,
              padding: isMobile ? 16 : 24, width: isMobile ? '95vw' : 480, maxWidth: '95vw', maxHeight: '90vh', overflowY: 'auto' as const,
            }}
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="bootstrap-modal-title"
            onKeyDown={e => { if (e.key === 'Escape' && !bootstrapLoading && !cloneLoading) setShowBootstrap(false); }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
              <h3 id="bootstrap-modal-title" style={{ margin: 0, color: '#c8ff00', fontSize: 16 }}>{t('projects.bootstrap')}</h3>
              <button onClick={() => setShowBootstrap(false)} style={{ background: 'none', border: 'none', color: 'rgba(255,255,255,0.5)', cursor: 'pointer' }} aria-label="Close">
                <X size={16} />
              </button>
            </div>
            <div style={{ display: 'flex', marginBottom: 16, borderRadius: 8, overflow: 'hidden', border: '1px solid rgba(255,255,255,0.1)' }}>
              <button
                onClick={() => setNewProjectMode('bootstrap')}
                style={{
                  flex: 1, padding: '8px 12px', border: 'none', fontSize: 13, fontFamily: 'inherit', cursor: 'pointer',
                  background: newProjectMode === 'bootstrap' ? 'rgba(200,255,0,0.15)' : 'transparent',
                  color: newProjectMode === 'bootstrap' ? '#c8ff00' : 'rgba(255,255,255,0.5)',
                }}
              >
                <Rocket size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                Bootstrap
              </button>
              <button
                onClick={() => setNewProjectMode('clone')}
                style={{
                  flex: 1, padding: '8px 12px', border: 'none', fontSize: 13, fontFamily: 'inherit', cursor: 'pointer',
                  borderLeft: '1px solid rgba(255,255,255,0.1)',
                  background: newProjectMode === 'clone' ? 'rgba(200,255,0,0.15)' : 'transparent',
                  color: newProjectMode === 'clone' ? '#c8ff00' : 'rgba(255,255,255,0.5)',
                }}
              >
                <Folder size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                {t('projects.clone')}
              </button>
            </div>
            {newProjectMode === 'bootstrap' && (
              <div onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && bootstrapName.trim() && bootstrapDesc.trim() && !bootstrapLoading) handleBootstrap(); }}>
                <label style={{ display: 'block', marginBottom: 12 }}>
                  <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', display: 'block', marginBottom: 4 }}>{t('projects.bootstrap.name')}</span>
                  <input
                    value={bootstrapName} onChange={e => setBootstrapName(e.target.value)}
                    style={{
                      width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.1)',
                      background: '#0d1117', color: '#e6edf3', fontSize: 14, fontFamily: 'inherit', boxSizing: 'border-box',
                    }}
                    placeholder="my-awesome-project"
                    autoFocus
                  />
                </label>
                <label style={{ display: 'block', marginBottom: 16 }}>
                  <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', display: 'block', marginBottom: 4 }}>{t('projects.bootstrap.desc')}</span>
                  <textarea
                    value={bootstrapDesc} onChange={e => setBootstrapDesc(e.target.value)}
                    rows={5}
                    style={{
                      width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.1)',
                      background: '#0d1117', color: '#e6edf3', fontSize: 13, fontFamily: 'inherit', resize: 'vertical',
                      boxSizing: 'border-box',
                    }}
                    placeholder={t('projects.bootstrap.descPlaceholder')}
                  />
                </label>

                {/* MCP selector (collapsible) */}
                {mcpOverview.configs.length > 0 && (
                  <div style={{ marginBottom: 16 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
                      <Server size={12} style={{ color: 'rgba(255,255,255,0.4)' }} />
                      <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)' }}>{t('projects.bootstrap.mcps')}</span>
                      {bootstrapMcpIds.length > 0 && (
                        <span style={{ fontSize: 10, color: '#c8ff00' }}>({bootstrapMcpIds.length})</span>
                      )}
                    </div>
                    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                      {mcpOverview.configs.map(cfg => {
                        const selected = bootstrapMcpIds.includes(cfg.id);
                        return (
                          <button
                            key={cfg.id}
                            type="button"
                            onClick={() => setBootstrapMcpIds(prev =>
                              selected ? prev.filter(id => id !== cfg.id) : [...prev, cfg.id]
                            )}
                            style={{
                              padding: '3px 8px', borderRadius: 10, fontSize: 10, fontFamily: 'inherit',
                              fontWeight: selected ? 600 : 400, cursor: 'pointer',
                              border: selected ? '1px solid rgba(0,212,255,0.4)' : '1px solid rgba(255,255,255,0.1)',
                              background: selected ? 'rgba(0,212,255,0.1)' : 'rgba(255,255,255,0.03)',
                              color: selected ? '#00d4ff' : 'rgba(255,255,255,0.5)',
                              display: 'flex', alignItems: 'center', gap: 3,
                              transition: 'all 0.15s',
                            }}
                          >
                            {selected && <Check size={9} />}
                            {cfg.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                <button
                  onClick={handleBootstrap}
                  disabled={bootstrapLoading || !bootstrapName.trim() || !bootstrapDesc.trim()}
                  style={{
                    width: '100%', padding: '10px 16px', borderRadius: 8, border: 'none',
                    background: bootstrapLoading || !bootstrapName.trim() || !bootstrapDesc.trim() ? 'rgba(200,255,0,0.15)' : '#c8ff00',
                    color: bootstrapLoading || !bootstrapName.trim() || !bootstrapDesc.trim() ? 'rgba(200,255,0,0.4)' : '#0a0c10',
                    fontSize: 14, fontWeight: 600, fontFamily: 'inherit', cursor: bootstrapLoading ? 'wait' : 'pointer',
                    display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8,
                  }}
                >
                  {bootstrapLoading ? <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /> : <Zap size={16} />}
                  {bootstrapLoading ? t('projects.bootstrap.creating') : t('projects.bootstrap.start')}
                </button>
              </div>
            )}
            {newProjectMode === 'clone' && (
              <div onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && cloneUrl.trim() && !cloneLoading) handleClone(); }}>
                <label style={{ display: 'block', marginBottom: 12 }}>
                  <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', display: 'block', marginBottom: 4 }}>{t('projects.clone.url')}</span>
                  <input
                    value={cloneUrl} onChange={e => setCloneUrl(e.target.value)}
                    style={{
                      width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.1)',
                      background: '#0d1117', color: '#e6edf3', fontSize: 14, fontFamily: 'inherit', boxSizing: 'border-box',
                    }}
                    placeholder={t('projects.clone.urlPlaceholder')}
                    autoFocus
                  />
                </label>
                <label style={{ display: 'block', marginBottom: 16 }}>
                  <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', display: 'block', marginBottom: 4 }}>{t('projects.clone.name')}</span>
                  <input
                    value={cloneName} onChange={e => setCloneName(e.target.value)}
                    style={{
                      width: '100%', padding: '8px 12px', borderRadius: 6, border: '1px solid rgba(255,255,255,0.1)',
                      background: '#0d1117', color: '#e6edf3', fontSize: 14, fontFamily: 'inherit', boxSizing: 'border-box',
                    }}
                    placeholder="my-project"
                  />
                </label>
                <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
                  <button
                    onClick={handleClone}
                    disabled={cloneLoading || !cloneUrl.trim()}
                    style={{
                      flex: 1, padding: '10px 16px', borderRadius: 8, border: 'none',
                      background: cloneLoading || !cloneUrl.trim() ? 'rgba(200,255,0,0.15)' : '#c8ff00',
                      color: cloneLoading || !cloneUrl.trim() ? 'rgba(200,255,0,0.4)' : '#0a0c10',
                      fontSize: 14, fontWeight: 600, fontFamily: 'inherit', cursor: cloneLoading ? 'wait' : 'pointer',
                      display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 8,
                    }}
                  >
                    {cloneLoading ? <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /> : <Folder size={16} />}
                    {cloneLoading ? t('projects.clone.cloning') : t('projects.clone.start')}
                  </button>
                  <button
                    onClick={() => handleDiscoverRepos()}
                    disabled={discoverLoading}
                    style={{
                      padding: '10px 16px', borderRadius: 8, border: '1px solid rgba(200,255,0,0.3)',
                      background: 'transparent', color: '#c8ff00',
                      fontSize: 13, fontWeight: 500, fontFamily: 'inherit', cursor: discoverLoading ? 'wait' : 'pointer',
                      display: 'flex', alignItems: 'center', gap: 6, whiteSpace: 'nowrap',
                    }}
                  >
                    {discoverLoading ? <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> : <Search size={14} />}
                    {discoverLoading ? t('projects.clone.discovering') : t('projects.clone.discover')}
                  </button>
                </div>

                {availableSources.length > 1 && (
                  <div style={{ marginBottom: 12 }}>
                    <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 6 }}>{t('projects.clone.selectSources')}</div>
                    <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                      {availableSources.map(src => (
                        <button
                          key={src.id}
                          onClick={() => toggleSource(src.id)}
                          style={{
                            padding: '4px 10px', borderRadius: 12, fontSize: 11, fontFamily: 'inherit',
                            border: '1px solid',
                            borderColor: selectedSourceIds.includes(src.id) ? 'rgba(200,255,0,0.4)' : 'rgba(255,255,255,0.1)',
                            background: selectedSourceIds.includes(src.id) ? 'rgba(200,255,0,0.1)' : 'transparent',
                            color: selectedSourceIds.includes(src.id) ? '#c8ff00' : 'rgba(255,255,255,0.4)',
                            cursor: 'pointer',
                            display: 'flex', alignItems: 'center', gap: 4,
                          }}
                        >
                          {src.provider === 'github' ? '🐙' : '🦊'} {src.label}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {discoverError && (
                  <div style={{ padding: 10, borderRadius: 8, background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', fontSize: 12, color: '#ff8a9e', marginBottom: 12 }}>
                    {discoverError}
                  </div>
                )}

                {discoverSources.length > 0 && (
                  <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginBottom: 8 }}>
                    {t('projects.clone.discovered').replace('{0}', String(discoveredRepos.length)).replace('{1}', discoverSources.join(', '))}
                  </div>
                )}

                {discoveredRepos.length > 0 && (
                  <div>
                    {discoveredRepos.length > 10 && (
                      <div style={{ marginBottom: 8 }}>
                        <input
                          type="text"
                          value={repoSearch}
                          onChange={e => setRepoSearch(e.target.value)}
                          placeholder={t('projects.clone.searchRepos')}
                          style={{
                            width: '100%', padding: '6px 10px', borderRadius: 6,
                            border: '1px solid rgba(255,255,255,0.1)', background: 'rgba(255,255,255,0.04)',
                            color: '#e6edf3', fontSize: 12, fontFamily: 'inherit',
                            boxSizing: 'border-box',
                          }}
                        />
                      </div>
                    )}
                  <div style={{ maxHeight: 300, overflowY: 'auto', borderRadius: 8, border: '1px solid rgba(255,255,255,0.06)' }}>
                    {discoveredRepos
                      .filter(repo => {
                        if (!repoSearch.trim()) return true;
                        const q = repoSearch.toLowerCase();
                        return repo.full_name.toLowerCase().includes(q)
                          || (repo.description ?? '').toLowerCase().includes(q)
                          || (repo.language ?? '').toLowerCase().includes(q);
                      })
                      .map(repo => (
                      <div
                        key={repo.full_name}
                        style={{
                          padding: '10px 12px', borderBottom: '1px solid rgba(255,255,255,0.04)',
                          display: 'flex', alignItems: 'center', gap: 10,
                          opacity: repo.already_cloned ? 0.5 : 1,
                        }}
                      >
                        <div style={{ flex: 1, minWidth: 0 }}>
                          <div style={{ fontSize: 13, color: '#e6edf3', fontWeight: 500 }}>
                            {repo.full_name}
                            {repo.language && (
                              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginLeft: 8 }}>{repo.language}</span>
                            )}
                            {repo.stargazers_count > 0 && (
                              <span style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginLeft: 6 }}>&#9733; {repo.stargazers_count}</span>
                            )}
                          </div>
                          {repo.description && (
                            <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.4)', marginTop: 2, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                              {repo.description}
                            </div>
                          )}
                        </div>
                        {repo.already_cloned ? (
                          <span style={{ fontSize: 11, color: 'rgba(200,255,0,0.5)', whiteSpace: 'nowrap' }}>
                            {t('projects.clone.alreadyCloned')}
                          </span>
                        ) : (
                          <button
                            onClick={() => handleCloneDiscovered(repo)}
                            disabled={cloneLoading}
                            style={{
                              padding: '4px 12px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.3)',
                              background: 'transparent', color: '#c8ff00', fontSize: 12, fontFamily: 'inherit',
                              cursor: cloneLoading ? 'wait' : 'pointer', whiteSpace: 'nowrap',
                            }}
                          >
                            {t('projects.clone.start')}
                          </button>
                        )}
                      </div>
                    ))}
                  </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Content */}
      <main style={s.main}>

        {/* ════════ PROJETS ════════ */}
        {page === 'projects' && (<ErrorBoundary mode="zone" label="Projects">{(() => {
          const visibleProjects = projects.filter(p => !isHiddenPath(p.path));
          const hiddenProjects = projects.filter(p => isHiddenPath(p.path));
          const baseProjects = showHidden ? projects : visibleProjects;
          // Filter by search
          const searchLower = projectSearch.toLowerCase();
          const filteredProjects = projectSearch
            ? baseProjects.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower))
            : baseProjects;

          // Extract org/owner from repo_url for grouping (shared utility)
          const projGroup = (p: Project) => getProjectGroup(p, t('projects.group.local'), t('projects.group.other'));

          // Sort alphabetically within each group
          const sortedProjects = [...filteredProjects].sort((a, b) => {
            const groupA = projGroup(a);
            const groupB = projGroup(b);
            if (groupA !== groupB) return groupA.localeCompare(groupB);
            return a.name.localeCompare(b.name);
          });

          // Group projects
          const groupedProjects: { group: string; projects: Project[] }[] = [];
          for (const p of sortedProjects) {
            const group = projGroup(p);
            const last = groupedProjects[groupedProjects.length - 1];
            if (last && last.group === group) { last.projects.push(p); }
            else { groupedProjects.push({ group, projects: [p] }); }
          }

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
                    <span style={{ color: 'rgba(255,255,255,0.35)' }}> + {hiddenProjects.length} {hiddenProjects.length > 1 ? t('projects.hiddenPlural') : t('projects.hidden')}</span>
                  )}
                </p>
              </div>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                {hiddenProjects.length > 0 && (
                  <button style={s.iconBtn} onClick={() => setShowHidden(!showHidden)} title={showHidden ? t('projects.hideHidden') : t('projects.showHidden')} aria-label={showHidden ? t('projects.hideHidden') : t('projects.showHidden')}>
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
                  style={{ background: 'rgba(255,255,255,0.04)', border: '1px solid rgba(255,255,255,0.12)', borderRadius: 6, padding: '8px 12px 8px 32px', color: '#e8eaed', fontSize: 12, fontFamily: 'inherit', width: '100%', boxSizing: 'border-box' as const }}
                  placeholder={t('projects.search')}
                  value={projectSearch}
                  onChange={(e) => setProjectSearch(e.target.value)}
                />
                {projectSearch && (
                  <button
                    style={{ position: 'absolute', right: 8, top: '50%', transform: 'translateY(-50%)', background: 'none', border: 'none', cursor: 'pointer', color: 'rgba(255,255,255,0.3)', padding: 2 }}
                    onClick={() => setProjectSearch('')}
                    aria-label="Clear search"
                  >
                    <X size={12} />
                  </button>
                )}
              </div>
            )}

            {displayProjects.map((proj: Project, idx: number) => {
              const isOpen = expandedId === proj.id;
              const projHidden = isHiddenPath(proj.path);
              // Group header: show when first item or group changes
              const currentGroup = projGroup(proj);
              const prevGroup = idx > 0 ? projGroup(displayProjects[idx - 1]) : null;
              const showGroupHeader = !projectSearch && groupedProjects.length > 1 && currentGroup !== prevGroup;
              const groupColor = currentGroup === t('projects.group.local') ? 'rgba(255,255,255,0.3)' : `hsl(${Math.abs([...currentGroup].reduce((h, c) => h * 31 + c.charCodeAt(0), 0)) % 360}, 60%, 65%)`;
              const projDiscussions = discussionsByProject[proj.id] ?? [];
              const validationDisc = projDiscussions.find(d => d.title === 'Validation audit AI');
              const validationInProgress = !!validationDisc && proj.audit_status === 'Audited';
              const bootstrapDisc = projDiscussions.find(d => d.title.startsWith('Bootstrap: '));
              const bootstrapInProgress = !!bootstrapDisc && proj.audit_status === 'TemplateInstalled';
              const briefingDisc = projDiscussions.find(d => d.title.startsWith('Briefing'));
              // Briefing is "done" if: briefing_notes saved in DB AND template is installed (not reset),
              // or audit already past template stage
              const briefingDone = proj.audit_status !== 'NoTemplate' && (
                !!proj.briefing_notes ||
                proj.audit_status === 'Audited' || proj.audit_status === 'Validated'
              );
              const groupProjectCount = groupedProjects.find(g => g.group === currentGroup)?.projects.length ?? 0;
              return (
                <div key={proj.id}>
                {showGroupHeader && (() => {
                  const isCollapsed = collapsedGroups.has(currentGroup);
                  return (
                  <button
                    style={{
                      display: 'flex', alignItems: 'center', gap: 8, margin: idx === 0 ? '0 0 6px' : '16px 0 6px',
                      padding: '4px 0', cursor: 'pointer', userSelect: 'none',
                      background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left',
                    }}
                    onClick={() => setCollapsedGroups(prev => {
                      const next = new Set(prev);
                      if (next.has(currentGroup)) next.delete(currentGroup); else next.add(currentGroup);
                      return next;
                    })}
                    aria-expanded={!isCollapsed}
                  >
                    <ChevronDown size={14} style={{ color: groupColor, transform: isCollapsed ? 'rotate(-90deg)' : 'none', transition: 'transform 0.15s', flexShrink: 0 }} />
                    <div style={{ width: 3, height: 14, borderRadius: 2, background: groupColor }} />
                    <span style={{ fontSize: 11, fontWeight: 800, color: groupColor, letterSpacing: '0.03em' }}>
                      {currentGroup}
                    </span>
                    <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.55)' }}>
                      ({groupProjectCount})
                    </span>
                    <div style={{ flex: 1, height: 1, background: `${groupColor}20` }} />
                  </button>
                  );
                })()}
                {collapsedGroups.has(currentGroup) ? null : (
                <div id={`project-${proj.id}`} style={{ ...s.card(isOpen || !!auditState[proj.id]?.active), opacity: projHidden ? 0.5 : 1 }}>
                  <button style={s.cardHeader} onClick={() => setExpandedId(isOpen ? null : proj.id)} aria-expanded={isOpen}>
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
                        ) : (proj.audit_status === 'Bootstrapped' || proj.audit_status === 'Audited' || proj.audit_status === 'Validated') ? (
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
                        {/* Drift badge */}
                        {driftByProject[proj.id]?.stale_sections?.length > 0 && (
                          <span style={{
                            display: 'inline-flex', alignItems: 'center', gap: 3,
                            padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600,
                            background: 'rgba(255,180,0,0.1)', color: '#f0a020',
                            border: '1px solid rgba(255,180,0,0.15)',
                            cursor: 'pointer',
                          }}
                          title={driftByProject[proj.id].stale_sections.map(s => s.ai_file).join(', ')}
                          >
                            <AlertTriangle size={9} />
                            {t('audit.staleSections', String(driftByProject[proj.id].stale_sections.length))}
                          </span>
                        )}
                        {driftByProject[proj.id]?.stale_sections?.length > 0 && (
                          <button
                            style={{
                              padding: '2px 8px', borderRadius: 4, fontSize: 10, fontWeight: 500,
                              background: 'rgba(255,180,0,0.1)', color: '#f0a020',
                              border: '1px solid rgba(255,180,0,0.2)', cursor: 'pointer',
                              display: 'inline-flex', alignItems: 'center', gap: 3,
                            }}
                            onClick={(e) => {
                              e.stopPropagation();
                              startPartialAudit(proj.id, driftByProject[proj.id]);
                            }}
                            disabled={!!auditState[proj.id]?.active}
                            title={t('audit.updateStale', String(driftByProject[proj.id].stale_sections.length))}
                          >
                            <RefreshCw size={9} />
                            {t('audit.updateStale', String(driftByProject[proj.id].stale_sections.length))}
                          </button>
                        )}
                        {/* Audit date */}
                        {driftByProject[proj.id]?.audit_date && (
                          <span style={{ fontSize: 9, color: 'rgba(255,255,255,0.35)' }}>
                            {t('audit.auditDate', new Date(driftByProject[proj.id].audit_date!).toLocaleDateString())}
                          </span>
                        )}
                      </div>
                      <div style={s.projPath}>{proj.path}</div>
                    </div>
                    <div style={{ ...s.projMeta, ...(isMobile ? { flexWrap: 'wrap' } : {}) } as React.CSSProperties}>
                      <span style={s.metaItem}><Server size={12} /> {mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id)).length}</span>
                      <span style={s.metaItem}><MessageSquare size={12} /> {projDiscussions.length}</span>
                    </div>
                  </button>

                  {isOpen && (
                    <div style={s.cardBody} onClick={(e) => e.stopPropagation()}>
                      {/* ── 1. Discussions (open after audit, closed before) ── */}
                      <div style={s.section}>
                        <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'discussions')} aria-expanded={isSectionOpen(proj.id, 'discussions', proj.audit_status)}>
                          {isSectionOpen(proj.id, 'discussions', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                          <MessageSquare size={14} /> <span style={s.sectionTitle}>Discussions</span>
                          <span style={s.count}>{projDiscussions.length}</span>
                        </button>
                        {isSectionOpen(proj.id, 'discussions', proj.audit_status) && (
                          <>
                            {projDiscussions.slice(0, 3).map(disc => (
                              <div key={disc.id} style={s.row}>
                                <div style={{ position: 'relative' }}>
                                  <div aria-hidden="true" style={s.dot(true)} />
                                  <span style={{ position: 'absolute', width: 1, height: 1, padding: 0, margin: -1, overflow: 'hidden', clip: 'rect(0,0,0,0)', whiteSpace: 'nowrap', border: 0 }}>
                                    {t('config.enabled')}
                                  </span>
                                </div>
                                <div style={{ flex: 1 }}>
                                  <span style={{ fontWeight: 600, fontSize: 12, display: 'inline-flex', alignItems: 'center', gap: 4 }}>
                                    {isValidationDisc(disc.title) && <ShieldCheck size={10} style={{ color: '#c8ff00' }} />}
                                    {disc.title}
                                  </span>
                                  <span style={{ marginLeft: 8, fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>
                                    {disc.message_count ?? disc.messages.length} msg · {disc.agent}
                                  </span>
                                </div>
                                <button style={s.iconBtn} onClick={() => { setPage('discussions'); }} aria-label="Open discussion">
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
                          </>
                        )}
                      </div>

                      {/* ── 2. Documentation AI (closed, only when validated) ── */}
                      {proj.audit_status === 'Validated' && (
                        <div style={s.section}>
                          <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'docAi')} aria-expanded={isSectionOpen(proj.id, 'docAi', proj.audit_status)}>
                            {isSectionOpen(proj.id, 'docAi', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                            <BookOpen size={14} /> <span style={s.sectionTitle}>{t('projects.docAi')}</span>
                          </button>
                          {isSectionOpen(proj.id, 'docAi', proj.audit_status) && (
                            <AiDocViewer
                              projectId={proj.id}
                              onDiscussFile={(filePath) => {
                                setDiscPrefill({
                                  projectId: proj.id,
                                  title: `Doc: ${filePath.replace('ai/', '')}`,
                                  prompt: t('projects.docAi.discussPrompt', filePath),
                                });
                                setPage('discussions');
                              }}
                            />
                          )}
                        </div>
                      )}

                      {/* ── 3. MCPs (closed) ── */}
                      {(() => {
                        const projMcps = mcpOverview.configs.filter(c => c.is_global || c.project_ids.includes(proj.id));
                        return (
                          <div style={s.section}>
                            <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'mcps')} aria-expanded={isSectionOpen(proj.id, 'mcps', proj.audit_status)}>
                              {isSectionOpen(proj.id, 'mcps', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                              <Server size={14} /> <span style={s.sectionTitle}>MCP</span>
                              <span style={s.count}>{projMcps.length}</span>
                            </button>
                            {isSectionOpen(proj.id, 'mcps', proj.audit_status) && (
                              <>
                                {projMcps.map(cfg => (
                                  <div key={cfg.id} style={s.row}>
                                    <div style={{ position: 'relative' }}>
                                      <div aria-hidden="true" style={s.dot(true)} />
                                      <span style={{ position: 'absolute', width: 1, height: 1, padding: 0, margin: -1, overflow: 'hidden', clip: 'rect(0,0,0,0)', whiteSpace: 'nowrap', border: 0 }}>
                                        {t('config.enabled')}
                                      </span>
                                    </div>
                                    <div style={{ flex: 1 }}>
                                      <span style={{ fontWeight: 600, fontSize: 12 }}>{cfg.server_name}</span>
                                      <span style={{ marginLeft: 6, fontSize: 11, color: 'rgba(255,255,255,0.4)' }}>{cfg.label}</span>
                                      {cfg.is_global && <span style={{ marginLeft: 4, fontSize: 9, color: '#c8ff00' }}>GLOBAL</span>}
                                    </div>
                                    <button
                                      style={s.iconBtn}
                                      onClick={() => setPage('mcps')}
                                      title={t('projects.manageMcps')}
                                      aria-label={t('projects.manageMcps')}
                                    >
                                      <ChevronRight size={12} />
                                    </button>
                                  </div>
                                ))}
                                {projMcps.length === 0 && (
                                  <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.55)', padding: '4px 0' }}>
                                    {t('projects.noMcp').split(' — ')[0]} — <button style={{ ...s.iconBtn, fontSize: 11, color: '#c8ff00' }} onClick={() => setPage('mcps')}>{t('projects.noMcp').split(' — ')[1]}</button>
                                  </div>
                                )}
                              </>
                            )}
                          </div>
                        );
                      })()}

                      {/* ── 4. Workflows (closed) ── */}
                      {(() => {
                        const projWorkflows = (workflowList ?? []).filter(w => w.project_id === proj.id);
                        return (
                          <div style={s.section}>
                            <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'workflows')} aria-expanded={isSectionOpen(proj.id, 'workflows', proj.audit_status)}>
                              {isSectionOpen(proj.id, 'workflows', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                              <Workflow size={14} /> <span style={s.sectionTitle}>{t('projects.workflows')}</span>
                              <span style={s.count}>{projWorkflows.length}</span>
                            </button>
                            {isSectionOpen(proj.id, 'workflows', proj.audit_status) && (
                              <>
                                {projWorkflows.map(wf => (
                                  <div key={wf.id} style={s.row}>
                                    <div style={{ position: 'relative' }}>
                                      <div aria-hidden="true" style={s.dot(wf.enabled)} />
                                      <span style={{ position: 'absolute', width: 1, height: 1, padding: 0, margin: -1, overflow: 'hidden', clip: 'rect(0,0,0,0)', whiteSpace: 'nowrap', border: 0 }}>
                                        {wf.enabled ? t('config.enabled') : t('config.disabled')}
                                      </span>
                                    </div>
                                    <div style={{ flex: 1 }}>
                                      <span style={{ fontWeight: 600, fontSize: 12 }}>{wf.name}</span>
                                      <span style={{ marginLeft: 6, fontSize: 10, color: 'rgba(255,255,255,0.4)' }}>
                                        {wf.trigger_type} · {wf.step_count} step{wf.step_count > 1 ? 's' : ''}
                                      </span>
                                      {wf.last_run && (
                                        <span style={{ marginLeft: 6, fontSize: 10, color: STATUS_COLORS[wf.last_run.status] ?? '#888' }}>
                                          {wf.last_run.status}
                                        </span>
                                      )}
                                    </div>
                                    <button
                                      style={s.iconBtn}
                                      onClick={() => setPage('workflows')}
                                      title={t('projects.workflows')}
                                      aria-label={t('projects.workflows')}
                                    >
                                      <ChevronRight size={12} />
                                    </button>
                                  </div>
                                ))}
                                {projWorkflows.length === 0 && (
                                  <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.55)', padding: '4px 0' }}>
                                    {t('projects.noWorkflows').split(' — ')[0]} — <button style={{ ...s.iconBtn, fontSize: 11, color: '#c8ff00' }} onClick={() => setPage('workflows')}>{t('projects.noWorkflows').split(' — ')[1]}</button>
                                  </div>
                                )}
                              </>
                            )}
                          </div>
                        );
                      })()}

                      {/* ── 5. Skills (closed) ── */}
                      <div style={s.section}>
                        <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'skills')} aria-expanded={isSectionOpen(proj.id, 'skills', proj.audit_status)}>
                          {isSectionOpen(proj.id, 'skills', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                          <Zap size={14} /> <span style={s.sectionTitle}>{t('projects.skills')}</span>
                          <span style={s.count}>{(proj.default_skill_ids ?? []).length}</span>
                        </button>
                        {isSectionOpen(proj.id, 'skills', proj.audit_status) && (
                          <div style={{ paddingTop: 6 }}>
                            <ProjectSkills
                              projectId={proj.id}
                              currentSkillIds={proj.default_skill_ids ?? []}
                              allSkills={allSkills}
                              onUpdate={() => { refetch(); refetchSkills(); }}
                            />
                          </div>
                        )}
                      </div>

                      {/* ── 6. AI Context / Audit (closed) ── */}
                      <div style={s.section}>
                        <button style={s.collapsibleHeader} onClick={() => toggleSection(proj.id, 'aiContext')} aria-expanded={isSectionOpen(proj.id, 'aiContext', proj.audit_status)}>
                          {isSectionOpen(proj.id, 'aiContext', proj.audit_status) ? <ChevronDown size={12} style={{ flexShrink: 0 }} /> : <ChevronRight size={12} style={{ flexShrink: 0 }} />}
                          <FileCode size={14} /> <span style={s.sectionTitle}>AI Context</span>
                          <span style={s.count}>
                            {proj.audit_status === 'Validated' ? t('projects.status.valid') : validationInProgress ? t('projects.status.validating') : proj.audit_status === 'Audited' ? t('projects.status.auditOk') : proj.audit_status === 'Bootstrapped' ? t('projects.status.bootstrapped') : bootstrapInProgress ? t('projects.status.bootstrapping') : proj.audit_status === 'TemplateInstalled' ? t('projects.status.template') : t('projects.status.none')}
                          </span>
                        </button>
                        {isSectionOpen(proj.id, 'aiContext', proj.audit_status) && (
                          <>
                            {(proj.audit_status === 'NoTemplate' || (proj.audit_status === 'TemplateInstalled' && !bootstrapInProgress)) && !auditState[proj.id]?.active && (
                              <div style={{ padding: '8px 0' }}>
                                <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 4 }}>
                                  <AlertTriangle size={11} /> {proj.audit_status === 'NoTemplate' ? t('audit.noTemplate') : t('audit.description')}
                                </p>
                                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                                  {briefingDisc && !briefingDone ? (
                                    <button
                                      style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#60a5fa', borderColor: 'rgba(96,165,250,0.2)' }}
                                      onClick={() => { setOpenDiscussionId(briefingDisc.id); setPage('discussions'); }}
                                    >
                                      <MessageSquare size={12} /> {t('audit.resumeBriefing')}
                                    </button>
                                  ) : !briefingDone ? (
                                    <button
                                      style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#60a5fa', borderColor: 'rgba(96,165,250,0.2)' }}
                                      onClick={async () => {
                                        const agent = auditAgentChoice[proj.id] ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode';
                                        try {
                                          const { discussion_id } = await projectsApi.startBriefing(proj.id, agent);
                                          refetchDiscussions();
                                          setAutoRunDiscussionId(discussion_id);
                                          setPage('discussions');
                                        } catch (err) {
                                          toast(String(err), 'error');
                                        }
                                      }}
                                      disabled={agents.filter(canAudit).length === 0}
                                    >
                                      <MessageSquare size={12} /> {t('audit.startBriefing')}
                                    </button>
                                  ) : (
                                    <span style={{ fontSize: 10, color: 'rgba(96,165,250,0.6)', display: 'flex', alignItems: 'center', gap: 4 }}>
                                      <Check size={10} /> {t('audit.briefingDone')}
                                    </span>
                                  )}
                                  {!briefingDone && (
                                    <span style={{ fontSize: 10, color: 'rgba(255,255,255,0.35)' }}>
                                      {t('audit.briefingDesc')}
                                    </span>
                                  )}
                                </div>
                                <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.55)', margin: '0 0 8px' }}>
                                  {t('audit.fullAuditDesc')}
                                </p>
                                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                                  <select
                                    style={{ ...auditSelectStyle, width: 'auto', minWidth: 140, fontSize: 12, padding: '6px 32px 6px 10px' }}
                                    value={auditAgentChoice[proj.id] ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                                    onChange={e => setAuditAgentChoice(prev => ({ ...prev, [proj.id]: e.target.value as AgentType }))}
                                  >
                                    {agents.filter(canAudit).map(a => (
                                      <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                                    ))}
                                    {agents.filter(canAudit).length === 0 && (
                                      <option value="" disabled>{t('disc.noAgent')}</option>
                                    )}
                                  </select>
                                  <button
                                    style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                    onClick={() => handleFullAudit(proj.id)}
                                    disabled={agents.filter(canAudit).length === 0}
                                  >
                                    <Play size={12} /> {t('audit.startFullAudit')}
                                  </button>
                                </div>
                              </div>
                            )}

                            {auditState[proj.id]?.active && (
                              <div style={{ padding: '8px 0' }}>
                                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
                                  <Loader2 size={14} style={{ animation: 'spin 1s linear infinite', color: '#c8ff00' }} />
                                  <span style={{ fontSize: 12, color: 'rgba(255,255,255,0.6)', flex: 1 }}>
                                    {t('audit.step', auditState[proj.id].step, auditState[proj.id].totalSteps, auditState[proj.id].currentFile)}
                                  </span>
                                  <button
                                    style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#ff4444', borderColor: 'rgba(255,68,68,0.3)', padding: '4px 10px' }}
                                    onClick={() => handleCancelAudit(proj.id)}
                                    title={t('audit.cancelAudit')}
                                  >
                                    <StopCircle size={12} /> {t('audit.cancelAudit')}
                                  </button>
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

                            {bootstrapInProgress && !auditState[proj.id]?.active && (
                              <div style={{ padding: '8px 0' }}>
                                <p style={{ fontSize: 11, color: 'rgba(255,200,0,0.7)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 4 }}>
                                  <Loader2 size={11} style={{ animation: 'spin 1s linear infinite' }} /> {t('audit.bootstrapInProgress')}
                                </p>
                                <button
                                  style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                  onClick={() => { setOpenDiscussionId(bootstrapDisc!.id); setPage('discussions'); }}
                                >
                                  <MessageSquare size={12} /> {t('audit.resumeBootstrap')}
                                </button>
                              </div>
                            )}

                            {proj.audit_status === 'Bootstrapped' && !auditState[proj.id]?.active && (
                              <div style={{ padding: '8px 0' }}>
                                <p style={{ fontSize: 11, color: 'rgba(200,255,0,0.5)', margin: '0 0 8px', display: 'flex', alignItems: 'center', gap: 4 }}>
                                  <Rocket size={11} /> {t('audit.bootstrapDone')}
                                </p>
                                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                                  <select
                                    style={{ ...auditSelectStyle, width: 'auto', minWidth: 140, fontSize: 12, padding: '6px 32px 6px 10px' }}
                                    value={auditAgentChoice[proj.id] ?? agents.filter(canAudit)[0]?.agent_type ?? 'ClaudeCode'}
                                    onChange={e => setAuditAgentChoice(prev => ({ ...prev, [proj.id]: e.target.value as AgentType }))}
                                  >
                                    {agents.filter(canAudit).map(a => (
                                      <option key={a.agent_type} value={a.agent_type}>{a.name}</option>
                                    ))}
                                    {agents.filter(canAudit).length === 0 && (
                                      <option value="" disabled>{t('disc.noAgent')}</option>
                                    )}
                                  </select>
                                  <button
                                    style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                    onClick={() => handleFullAudit(proj.id)}
                                    disabled={agents.filter(canAudit).length === 0}
                                  >
                                    <Play size={12} /> {t('audit.startFullAudit')}
                                  </button>
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
                                    <p style={{ fontSize: 10, color: 'rgba(255,255,255,0.55)', margin: '0 0 8px' }}>
                                      {t('audit.validationHint')}
                                    </p>
                                    <button
                                      style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                      onClick={() => { setOpenDiscussionId(validationDisc!.id); setPage('discussions'); }}
                                    >
                                      <MessageSquare size={12} /> {t('audit.resumeValidation')}
                                    </button>
                                  </>
                                ) : (
                                  <>
                                    <p style={{ fontSize: 11, color: 'rgba(255,255,255,0.6)', margin: '0 0 8px' }}>
                                      {t('audit.readyToValidate')}
                                    </p>
                                    <button
                                      style={{ ...s.iconBtn, fontSize: 11, gap: 4, color: '#c8ff00', borderColor: 'rgba(200,255,0,0.2)' }}
                                      onClick={() => {
                                        setDiscPrefill({
                                          projectId: proj.id,
                                          title: 'Validation audit AI',
                                          prompt: t('audit.validationPrompt'),
                                          locked: true,
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
                          </>
                        )}
                      </div>

                      <div style={{ marginTop: 12, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,0.05)' }}>
                        {deleteConfirmId === proj.id ? (
                          <div>
                            <div style={{ display: 'flex', gap: 8, marginBottom: 8 }}>
                              <button
                                style={{ ...s.scanBtn, fontSize: 12, padding: '6px 12px', background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(255,255,255,0.1)', color: 'rgba(255,255,255,0.7)' }}
                                onClick={() => handleDeleteProject(proj.id, false)}
                              >
                                {t('projects.deleteSoft')}
                              </button>
                            </div>
                            <div style={{ background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 8, padding: 10 }}>
                              <div style={{ fontSize: 11, color: '#ff4d6a', marginBottom: 8 }}>
                                <AlertTriangle size={12} style={{ verticalAlign: 'middle', marginRight: 4 }} />
                                {t('projects.deleteHardWarn')}
                              </div>
                              <div style={{ fontSize: 11, color: 'rgba(255,255,255,0.5)', marginBottom: 6 }}>{t('projects.deleteHardConfirmLabel')}</div>
                              <input
                                value={deleteConfirmInput}
                                onChange={e => setDeleteConfirmInput(e.target.value)}
                                placeholder={proj.name}
                                style={{
                                  width: '100%', padding: '6px 10px', borderRadius: 6, border: '1px solid rgba(255,77,106,0.3)',
                                  background: '#0d1117', color: '#e6edf3', fontSize: 12, fontFamily: 'inherit', boxSizing: 'border-box', marginBottom: 8,
                                }}
                              />
                              <div style={{ display: 'flex', gap: 8 }}>
                                <button
                                  style={{ ...s.dangerBtn, opacity: deleteConfirmInput === proj.name ? 1 : 0.4, pointerEvents: deleteConfirmInput === proj.name ? 'auto' : 'none' }}
                                  onClick={() => handleDeleteProject(proj.id, true)}
                                  disabled={deleteConfirmInput !== proj.name}
                                >
                                  <Trash2 size={12} /> {t('projects.deleteHard')}
                                </button>
                                <button
                                  style={{ ...s.scanBtn, fontSize: 12, padding: '6px 12px', background: 'rgba(255,255,255,0.06)', border: '1px solid rgba(255,255,255,0.1)', color: 'rgba(255,255,255,0.7)' }}
                                  onClick={() => { setDeleteConfirmId(null); setDeleteConfirmInput(''); }}
                                >
                                  {t('audit.cancelAudit')}
                                </button>
                              </div>
                            </div>
                          </div>
                        ) : (
                          <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                            <button style={s.dangerBtn} onClick={() => setDeleteConfirmId(proj.id)}>
                              <Trash2 size={12} /> {t('projects.delete')}
                            </button>
                          </div>
                        )}
                      </div>
                    </div>
                  )}
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
        })()}</ErrorBoundary>)}

        {/* ════════ MCPs ════════ */}
        {page === 'mcps' && (
          <ErrorBoundary mode="zone" label="MCPs">
            <McpPage projects={projects} mcpOverview={mcpOverview} mcpRegistry={mcpRegistry} refetchMcps={refetchMcps} />
          </ErrorBoundary>
        )}

        {/* ════════ WORKFLOWS ════════ */}
        {page === 'workflows' && (
          <ErrorBoundary mode="zone" label="Workflows">
            <WorkflowsPage projects={projects} installedAgentTypes={agents.filter(isUsable).map(a => a.agent_type)} agentAccess={agentAccess ?? undefined} />
          </ErrorBoundary>
        )}

        {/* ════════ DISCUSSIONS ════════ */}
        {page === 'discussions' && (
          <ErrorBoundary mode="zone" label="Discussions">
          <DiscussionsPage
            projects={projects}
            agents={agents}
            allDiscussions={allDiscussions}
            configLanguage={configLanguage ?? null}
            agentAccess={agentAccess ?? null}
            refetchDiscussions={refetchDiscussions}
            refetchProjects={refetch}
            onNavigate={(p, opts) => {
              setPage(p as Page);
              if (opts?.projectId) {
                setExpandedId(opts.projectId);
                setOpenSections(prev => ({ ...prev, [opts.projectId!]: 'aiContext' }));
                setTimeout(() => {
                  document.getElementById(`project-${opts.projectId}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
                }, 100);
              }
              if (opts?.scrollTo) {
                setTimeout(() => {
                  document.getElementById(opts.scrollTo!)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
                }, 200);
              }
            }}
            prefill={discPrefill}
            onPrefillConsumed={handlePrefillConsumed}
            autoRunDiscussionId={autoRunDiscussionId}
            onAutoRunConsumed={handleAutoRunConsumed}
            openDiscussionId={openDiscussionId}
            onOpenDiscConsumed={handleOpenDiscConsumed}
            toast={toast}
            sendingMap={sendingMap}
            setSendingMap={setSendingMap}
            sendingStartMap={sendingStartMap}
            setSendingStartMap={setSendingStartMap}
            streamingMap={streamingMap}
            setStreamingMap={setStreamingMap}
            abortControllers={abortControllers}
            cleanupStream={cleanupStream}
            markDiscussionSeen={markDiscussionSeen}
            onActiveDiscussionChange={setActiveDiscussionId}
            initialActiveDiscussionId={activeDiscussionId}
            lastSeenMsgCount={lastSeenMsgCount}
            mcpConfigs={mcpOverview.configs}
            mcpIncompatibilities={mcpOverview.incompatibilities}
          />
          </ErrorBoundary>
        )}

        {/* ════════ CONFIG ════════ */}
        {page === 'settings' && (
          <ErrorBoundary mode="zone" label="Settings">
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
          </ErrorBoundary>
        )}
      </main>
    </div>
  );
}

// ─── Styles ─────────────────────────────────────────────────────────────────

// Standalone styled select for audit agent chooser (was ds.selectStyled)
const auditSelectStyle = {
  width: '100%', padding: '9px 12px', background: '#1a1d26', border: '1px solid rgba(255,255,255,0.12)',
  borderRadius: 8, color: '#e8eaed', fontSize: 13, fontFamily: 'inherit',
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
  navBtn: (active: boolean) => ({ padding: '7px 14px', borderRadius: 6, border: 'none', background: active ? 'rgba(200,255,0,0.1)' : 'transparent', color: active ? '#c8ff00' : 'rgba(255,255,255,0.5)', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: active ? 600 : 400, display: 'flex', alignItems: 'center', gap: 6, transition: 'background 0.15s, color 0.15s' } as const),
  scanBtn: { padding: '7px 14px', borderRadius: 6, border: '1px solid rgba(200,255,0,0.25)', background: 'rgba(200,255,0,0.05)', color: '#c8ff00', cursor: 'pointer', fontSize: 12, fontFamily: 'inherit', fontWeight: 500, display: 'flex', alignItems: 'center', gap: 6, transition: 'background 0.15s, border-color 0.15s' } as const,
  main: { maxWidth: 1000, margin: '0 auto', padding: '20px 16px' } as const,
  mainFull: { margin: 0, padding: '24px 20px', flex: 1 } as const,
  pageHeader: { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 20 } as const,
  h1: { fontSize: 20, fontWeight: 700, letterSpacing: '-0.02em' } as const,
  meta: { color: 'rgba(255,255,255,0.4)', fontSize: 12, marginTop: 4 } as const,
  card: (active: boolean) => ({ background: '#12151c', border: `1px solid ${active ? 'rgba(255,255,255,0.12)' : 'rgba(255,255,255,0.07)'}`, borderRadius: 10, marginBottom: 10, transition: 'border-color 0.2s' } as const),
  cardHeader: { display: 'flex', alignItems: 'center', gap: 12, padding: '16px 20px', cursor: 'pointer', background: 'none', border: 'none', width: '100%', font: 'inherit', color: 'inherit', textAlign: 'left' as const, borderRadius: 10, transition: 'background 0.15s' } as const,
  cardBody: { padding: '0 20px 20px', borderTop: '1px solid rgba(255,255,255,0.05)', overflowX: 'hidden' as const } as const,
  projName: { fontWeight: 600, fontSize: 14 } as const,
  projPath: { fontSize: 11, color: 'rgba(255,255,255,0.55)', fontFamily: 'JetBrains Mono, monospace', marginTop: 2 } as const,
  projMeta: { display: 'flex', gap: 14, fontSize: 11, color: 'rgba(255,255,255,0.4)', flexShrink: 0 } as const,
  metaItem: { display: 'flex', alignItems: 'center', gap: 4 } as const,
  section: { marginTop: 14 } as const,
  sectionHeader: { display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10, color: 'rgba(255,255,255,0.6)', fontSize: 12 } as const,
  collapsibleHeader: { display: 'flex', alignItems: 'center', gap: 8, marginBottom: 6, color: 'rgba(255,255,255,0.6)', fontSize: 12, cursor: 'pointer', userSelect: 'none' as const, padding: '4px 0', borderRadius: 4, background: 'none', border: 'none', width: '100%', font: 'inherit', textAlign: 'left' as const } as const,
  sectionTitle: { fontWeight: 600 } as const,
  sectionLabel: { fontSize: 11, fontWeight: 600, textTransform: 'uppercase' as const, letterSpacing: '0.05em', color: 'rgba(255,255,255,0.4)', marginBottom: 10 },
  count: { fontSize: 10, padding: '1px 6px', borderRadius: 10, background: 'rgba(255,255,255,0.06)', color: 'rgba(255,255,255,0.4)' } as const,
  row: { display: 'flex', alignItems: 'center', gap: 10, padding: '8px 12px', borderRadius: 6, background: 'rgba(255,255,255,0.02)', marginBottom: 4 } as const,
  dot: (on: boolean) => ({ width: 7, height: 7, borderRadius: '50%', background: on ? '#34d399' : 'rgba(255,255,255,0.15)', boxShadow: on ? '0 0 6px rgba(52,211,153,0.4)' : 'none', flexShrink: 0 } as const),
  badge: { display: 'inline-block', padding: '2px 8px', borderRadius: 4, fontSize: 10, background: 'rgba(255,255,255,0.06)', color: 'rgba(255,255,255,0.5)' } as const,
  badgeGreen: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(200,255,0,0.1)', color: 'rgba(200,255,0,0.7)', border: '1px solid rgba(200,255,0,0.15)' } as const,
  badgeOrange: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(255,200,0,0.08)', color: '#f59e0b', border: '1px solid rgba(245,158,11,0.2)' } as const,
  badgeGray: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(255,255,255,0.03)', color: 'rgba(255,255,255,0.55)', border: '1px solid rgba(255,255,255,0.05)' } as const,
  agentBadge: { display: 'inline-flex', alignItems: 'center', gap: 3, padding: '2px 7px', borderRadius: 4, fontSize: 10, fontWeight: 600, background: 'rgba(139,92,246,0.1)', color: 'rgba(139,92,246,0.7)', border: '1px solid rgba(139,92,246,0.15)' } as const,
  iconBtn: { background: 'none', border: '1px solid rgba(255,255,255,0.1)', borderRadius: 4, padding: '4px 8px', color: 'rgba(255,255,255,0.55)', cursor: 'pointer', display: 'flex', alignItems: 'center', fontSize: 11, transition: 'background 0.15s, border-color 0.15s' } as const,
  dangerBtn: { background: 'rgba(255,77,106,0.08)', border: '1px solid rgba(255,77,106,0.2)', borderRadius: 6, padding: '6px 14px', color: '#ff4d6a', cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontSize: 12, fontFamily: 'inherit' } as const,
  installBtn: { padding: '6px 14px', background: 'rgba(200,255,0,0.1)', color: '#c8ff00', border: '1px solid rgba(200,255,0,0.2)', borderRadius: 6, fontSize: 12, cursor: 'pointer', display: 'flex', alignItems: 'center', gap: 6, fontFamily: 'inherit', transition: 'background 0.15s, border-color 0.15s' } as const,
  input: { width: '100%', padding: '8px 12px', background: 'rgba(255,255,255,0.03)', border: '1px solid rgba(255,255,255,0.08)', borderRadius: 6, color: '#e8eaed', fontSize: 12, fontFamily: 'inherit' } as const,
  mcpDropdown: { marginTop: 4, borderRadius: 8, border: '1px solid rgba(255,255,255,0.08)', background: '#181c26', overflow: 'hidden' } as const,
  mcpOption: { display: 'flex', alignItems: 'center', gap: 8, padding: '10px 12px', cursor: 'pointer', fontSize: 12, borderBottom: '1px solid rgba(255,255,255,0.04)' } as const,
  empty: { padding: 16, textAlign: 'center' as const, color: 'rgba(255,255,255,0.55)', fontSize: 12 },
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
