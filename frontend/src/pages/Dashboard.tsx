import './Dashboard.css';
import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { projects as projectsApi, mcps as mcpsApi, agents as agentsApi, discussions as discussionsApi, workflows as workflowsApi, config as configApi, skills as skillsApi } from '../lib/api';
import { useApi } from '../hooks/useApi';
import { useToast } from '../hooks/useToast';
import type { RemoteRepo, RepoSource, DriftCheckResponse } from '../types/generated';
import { useT } from '../lib/I18nContext';
import { useIsMobile } from '../hooks/useMediaQuery';
import { isUsable } from '../lib/constants';
import { hydrateTtsVoicesFromBackend } from '../lib/tts-models';
import { userError } from '../lib/userError';
import { TourProvider } from '../components/tour/TourProvider';
import { TourOverlay } from '../components/tour/TourOverlay';
import { TourHelpButton } from '../components/tour/TourHelpButton';
import { fetchSttModelId } from '../lib/stt-models';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { McpPage } from './McpPage';
import { WorkflowsPage } from './WorkflowsPage';
import { SettingsPage } from './SettingsPage';
import { DiscussionsPage } from './DiscussionsPage';
import { ProjectList } from '../components/ProjectList';
import {
  Folder, FolderOpen, Puzzle,
  Plus, Search, Zap, Settings,
  Loader2,
  MessageSquare, X,
  Rocket, Check, Workflow, FileText,
} from 'lucide-react';

type Page = 'projects' | 'mcps' | 'workflows' | 'discussions' | 'settings';

interface DashboardProps {
  onReset: () => void;
}

/** Agents that can run audits/briefings (need filesystem access + CLI mode). Excludes Vibe (API-only). */
const canAudit = (a: { installed: boolean; runtime_available: boolean; enabled: boolean; agent_type: string }) => isUsable(a) && a.agent_type !== 'Vibe';



// Sort score for project readiness
export function Dashboard({ onReset }: DashboardProps) {
  const { t } = useT();
  const isMobile = useIsMobile();
  const { toast, ToastContainer } = useToast();
  const [page, setPage] = useState<Page>('projects');
  const [mcpSelectedConfigId, setMcpSelectedConfigId] = useState<string | null>(null);
  const [expandedId, setExpandedId] = useState<string | null>(null);
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
  // When the sidebar batch pastille is clicked, we hand the workflow id to
  // WorkflowsPage via this prop. It's cleared right after consumption so the
  // navigation only fires once per click.
  const [openWorkflowId, setOpenWorkflowId] = useState<string | null>(null);
  // Reverse direction: when a "📋 View N discussions" chip on a workflow run
  // is clicked, we hand the batch run id to DiscussionsPage so the sidebar
  // expands the matching batch group + scrolls to it.
  const [focusBatchId, setFocusBatchId] = useState<string | null>(null);

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

  const { data: projectList, initialLoading: projectsLoading, refetch } = useApi(() => projectsApi.list(), []);

  // ─── Deep-link: #project-<id> hash → auto-expand + scroll ──────────
  // Used by the CLI: `kronn` opens `http://localhost:3140/#project-<id>`
  // so the dashboard scrolls directly to the right project card.
  //
  // Timing: consumed AFTER `projectList` is loaded (not on mount) because
  // the ProjectCard DOM nodes don't exist until the fetch completes. A ref
  // guards against re-firing on subsequent refetches.
  const hashConsumedRef = useRef(false);
  useEffect(() => {
    if (hashConsumedRef.current) return;
    if (!projectList || projectList.length === 0) return;

    const hash = window.location.hash;
    if (!hash.startsWith('#project-')) return;
    const projectId = hash.slice('#project-'.length);
    if (!projectId) return;

    // Verify the project actually exists in the loaded list.
    if (!projectList.some(p => p.id === projectId)) return;

    hashConsumedRef.current = true;

    // Ensure we're on the Projects page (not Discussions / MCPs / etc.)
    setPage('projects');
    // Expand the card...
    setExpandedId(projectId);

    // ...then scroll after React re-renders with the card open. Two rAF
    // frames: one for React to commit the DOM, one for the browser to
    // layout the expanded card.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = document.getElementById(`project-${projectId}`);
        if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
    });

    // Clean the hash so a page refresh doesn't re-trigger.
    if (window.history.replaceState) {
      window.history.replaceState(null, '', window.location.pathname);
    }
  }, [projectList]); // eslint-disable-line react-hooks/exhaustive-deps

  const { data: registry } = useApi(() => mcpsApi.registry(), []);
  const { data: mcpOverviewData, refetch: refetchMcps } = useApi(() => mcpsApi.overview(), []);
  const { data: agentList, refetch: refetchAgents } = useApi(() => agentsApi.detect(), []);
  const { data: discussionList, refetch: refetchDiscussions } = useApi(() => discussionsApi.list(), []);
  const { data: configLanguage, refetch: refetchLanguage } = useApi(() => configApi.getLanguage(), []);
  const { data: agentAccess, refetch: refetchAgentAccess } = useApi(() => configApi.getAgentAccess(), []);
  const { data: workflowList, refetch: refetchWorkflows } = useApi(() => workflowsApi.list(), []);
  const { data: skillList, refetch: refetchSkills } = useApi(() => skillsApi.list(), []);

  // Hydrate user-preference caches from backend once at mount.
  // Tauri WebView2 can wipe localStorage across app updates, so the backend
  // is the durable source of truth for these settings. The calls are
  // best-effort — failure falls back silently to whatever localStorage holds.
  useEffect(() => {
    void fetchSttModelId();
    void hydrateTtsVoicesFromBackend();
  }, []);

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

  // Drift refetch callback for ProjectCard partial audit
  const handleRefetchDrift = useCallback((projectId: string) => {
    projectsApi.checkDrift(projectId).then(d => {
      if (d) setDriftByProject(prev => ({ ...prev, [projectId]: d }));
    }).catch(() => {});
  }, []);

  // Bootstrap new project state
  const [showBootstrap, setShowBootstrap] = useState(false);
  const [bootstrapName, setBootstrapName] = useState('');
  const [bootstrapDesc, setBootstrapDesc] = useState('');
  const [bootstrapLoading, setBootstrapLoading] = useState(false);
  const [bootstrapMcpIds, setBootstrapMcpIds] = useState<string[]>([]);
  const [bootstrapFiles, setBootstrapFiles] = useState<File[]>([]);
  const [bootstrapRepoMcp, setBootstrapRepoMcp] = useState('');       // MCP config ID for repo creation
  const [bootstrapTrackerMcp, setBootstrapTrackerMcp] = useState(''); // MCP config ID for issue tracker

  // When the bootstrap modal opens, pre-select the first available repo MCP
  // and tracker MCP so the user doesn't have to remember to pick them. They
  // can still opt out via the empty option in each dropdown. Only fires when
  // the field is empty so we don't override an explicit user choice.
  useEffect(() => {
    if (!showBootstrap) return;
    const repoMcps = mcpOverview.configs.filter(c =>
      c.server_id === 'mcp-github' || c.server_id === 'mcp-gitlab'
    );
    const trackerMcps = mcpOverview.configs.filter(c =>
      c.server_id === 'mcp-github' || c.server_id === 'mcp-atlassian' || c.server_id === 'mcp-linear'
    );
    if (!bootstrapRepoMcp && repoMcps.length > 0) {
      setBootstrapRepoMcp(repoMcps[0].id);
    }
    if (!bootstrapTrackerMcp && trackerMcps.length > 0) {
      setBootstrapTrackerMcp(trackerMcps[0].id);
    }
  }, [showBootstrap, mcpOverview.configs, bootstrapRepoMcp, bootstrapTrackerMcp]);
  const [newProjectMode, setNewProjectMode] = useState<'bootstrap' | 'clone' | 'folder'>('bootstrap');
  const [folderPath, setFolderPath] = useState('');
  const [folderName, setFolderName] = useState('');
  const [folderLoading, setFolderLoading] = useState(false);
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
      // Build description with bootstrap++ instructions
      let desc = bootstrapDesc.trim();
      const instructions: string[] = [];
      const repoMcpLabel = bootstrapRepoMcp ? mcpOverview.configs.find(c => c.id === bootstrapRepoMcp)?.label : null;
      const trackerMcpLabel = bootstrapTrackerMcp ? mcpOverview.configs.find(c => c.id === bootstrapTrackerMcp)?.label : null;
      if (repoMcpLabel) instructions.push(`Create a Git repository for this project via "${repoMcpLabel}" MCP.`);
      if (trackerMcpLabel) instructions.push(`After architecture validation, generate a full project plan and create issues via "${trackerMcpLabel}" MCP.`);
      if (bootstrapFiles.length > 0) instructions.push(`${bootstrapFiles.length} document(s) uploaded as context — read them carefully before starting.`);
      if (instructions.length > 0) desc += '\n\n---\nBootstrap instructions:\n' + instructions.map(i => `- ${i}`).join('\n');

      // Include selected MCPs in the config
      const allMcpIds = [...bootstrapMcpIds];
      if (bootstrapRepoMcp && !allMcpIds.includes(bootstrapRepoMcp)) allMcpIds.push(bootstrapRepoMcp);
      if (bootstrapTrackerMcp && !allMcpIds.includes(bootstrapTrackerMcp)) allMcpIds.push(bootstrapTrackerMcp);

      const res = await projectsApi.bootstrap({ name: bootstrapName.trim(), description: desc, agent, mcp_config_ids: allMcpIds, skill_ids: ['bootstrap-architect'] });
      // Upload context files to the bootstrap discussion
      for (const file of bootstrapFiles) {
        try {
          await discussionsApi.uploadContextFile(res.discussion_id, file);
        } catch (e) {
          console.warn('Failed to upload context file:', e);
        }
      }
      setShowBootstrap(false);
      setBootstrapName('');
      setBootstrapDesc('');
      setBootstrapMcpIds([]);
      setBootstrapFiles([]);
      setBootstrapRepoMcp('');
      setBootstrapTrackerMcp('');
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
      toast(userError(e), 'error');
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
      toast(userError(e), 'error');
    } finally {
      setCloneLoading(false);
    }
  };

  const [scanning, setScanning] = useState(false);
  const handleScan = async () => {
    if (scanning) return;
    setScanning(true);
    try {
      const repos = await projectsApi.scan();
      let added = 0;
      for (const repo of repos) {
        if (!repo.has_project && !repo.hidden) {
          await projectsApi.create(repo);
          added++;
        }
      }
      refetch();
      if (added > 0) toast(t('projects.scanFound', added), 'success');
      else toast(t('projects.scanNone'), 'info');
    } catch {
      toast(t('projects.scanError'), 'error');
    } finally {
      setScanning(false);
    }
  };

  return (
    <div className="dash-app">
      <ToastContainer />
      <TourProvider setPage={setPage as (p: string) => void}>
      {/* Nav */}
      <nav className="dash-nav">
        <div className="dash-nav-brand" data-mobile={isMobile}>
          <Zap size={18} className="text-accent" />
          {!isMobile && <span className="dash-nav-title">Kronn</span>}
          {isMobile && <span className="dash-nav-title-mobile">K</span>}
        </div>
        <div className="dash-nav-tabs" data-mobile={isMobile}>
        {([
          ['projects', Folder, t('nav.projects')],
          ['discussions', MessageSquare, t('nav.discussions')],
          ['mcps', Puzzle, t('nav.mcps')],
          ['workflows', Workflow, t('nav.workflows')],
          ['settings', Settings, t('nav.config')],
        ] as [string, typeof Folder, string][]).map(([id, Icon, label]) => (
          <button key={id} className="dash-nav-btn" data-active={page === id} data-mobile={isMobile} data-tour-id={`nav-${id}`} onClick={() => { setPage(id as Page); if (id !== 'mcps') setMcpSelectedConfigId(null); }} title={label}>
            {id === 'workflows' && runningWorkflows > 0
              ? <Loader2 size={isMobile ? 16 : 14} style={{ animation: 'spin 1s linear infinite' }} className="text-accent" />
              : <Icon size={isMobile ? 16 : 14} />
            }
            {!isMobile && <>{' '}{label}</>}
            {id === 'discussions' && totalUnseen > 0 && (
              <span className="dash-nav-badge">{totalUnseen}</span>
            )}
            {id === 'workflows' && runningWorkflows > 0 && (
              <span className="dash-nav-badge">{runningWorkflows}</span>
            )}
          </button>
        ))}
        </div>
        <div className="dash-nav-spacer" data-mobile={isMobile} />
        <button className="dash-scan-btn" data-tour-id="new-project-btn" onClick={() => setShowBootstrap(true)} title={t('projects.bootstrap')}>
          <Plus size={14} /> {!isMobile && t('projects.bootstrap')}
        </button>
        <button className="dash-scan-btn" data-tour-id="scan-btn" onClick={handleScan} disabled={scanning} title={t('nav.scan')}>
          {scanning ? <Loader2 size={14} className="spin" /> : <Search size={14} />}
          {!isMobile && (scanning ? t('projects.scanning') : t('nav.scan'))}
        </button>
        <TourHelpButton />
      </nav>

      {/* Bootstrap modal */}
      {showBootstrap && (
        <div className="dash-modal-overlay" onClick={() => !bootstrapLoading && !cloneLoading && setShowBootstrap(false)}>
          <div
            className="dash-modal"
            data-mobile={isMobile}
            onClick={e => e.stopPropagation()}
            role="dialog"
            aria-modal="true"
            aria-labelledby="bootstrap-modal-title"
            onKeyDown={e => { if (e.key === 'Escape' && !bootstrapLoading && !cloneLoading) setShowBootstrap(false); }}
          >
            <div className="dash-modal-header">
              <h3 id="bootstrap-modal-title" className="dash-modal-title">{t('projects.bootstrap')}</h3>
              <button onClick={() => setShowBootstrap(false)} className="dash-modal-close" aria-label="Close">
                <X size={16} />
              </button>
            </div>
            <div className="dash-tab-bar">
              <button
                onClick={() => setNewProjectMode('bootstrap')}
                className="dash-tab"
                data-active={newProjectMode === 'bootstrap'}
                data-tour-id="tab-bootstrap"
              >
                <Rocket size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                Bootstrap
              </button>
              <button
                onClick={() => setNewProjectMode('clone')}
                className="dash-tab"
                data-active={newProjectMode === 'clone'}
                data-tour-id="tab-clone"
              >
                <Folder size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                {t('projects.clone')}
              </button>
              <button
                onClick={() => setNewProjectMode('folder')}
                className="dash-tab"
                data-active={newProjectMode === 'folder'}
                data-tour-id="tab-folder"
              >
                <FolderOpen size={14} style={{ verticalAlign: 'middle', marginRight: 6 }} />
                {t('projects.addFolder')}
              </button>
            </div>
            {newProjectMode === 'bootstrap' && (
              <div onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && bootstrapName.trim() && bootstrapDesc.trim() && !bootstrapLoading) handleBootstrap(); }}>
                <label className="dash-field">
                  <span className="dash-field-label">{t('projects.bootstrap.name')}</span>
                  <input
                    value={bootstrapName} onChange={e => setBootstrapName(e.target.value)}
                    className="dash-field-input"
                    placeholder="my-awesome-project"
                    autoFocus
                  />
                </label>
                <label className="dash-field" style={{ marginBottom: 16 }}>
                  <span className="dash-field-label">{t('projects.bootstrap.desc')}</span>
                  <textarea
                    value={bootstrapDesc} onChange={e => setBootstrapDesc(e.target.value)}
                    rows={5}
                    className="dash-field-textarea"
                    placeholder={t('projects.bootstrap.descPlaceholder')}
                  />
                </label>

                {/* MCP selector (collapsible) */}
                {mcpOverview.configs.length > 0 && (
                  <div className="dash-mcp-selector">
                    <div className="dash-mcp-label">
                      <Puzzle size={12} className="text-muted" />
                      <span className="text-base text-secondary">{t('projects.bootstrap.mcps')}</span>
                      {bootstrapMcpIds.length > 0 && (
                        <span className="dash-mcp-count">({bootstrapMcpIds.length})</span>
                      )}
                    </div>
                    <div className="flex-wrap gap-2" style={{ display: 'flex' }}>
                      {mcpOverview.configs.map(cfg => {
                        const selected = bootstrapMcpIds.includes(cfg.id);
                        return (
                          <button
                            key={cfg.id}
                            type="button"
                            onClick={() => setBootstrapMcpIds(prev =>
                              selected ? prev.filter(id => id !== cfg.id) : [...prev, cfg.id]
                            )}
                            className="dash-mcp-chip"
                            data-selected={selected}
                          >
                            {selected && <Check size={9} />}
                            {cfg.label}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}

                {/* Document upload */}
                <div className="dash-file-upload" style={{ marginBottom: 16 }}>
                  <span className="dash-field-label">{t('projects.bootstrap.docs')}</span>
                  <div
                    className="dash-drop-zone"
                    onDragOver={e => { e.preventDefault(); e.currentTarget.dataset.dragover = 'true'; }}
                    onDragLeave={e => { e.currentTarget.dataset.dragover = 'false'; }}
                    onDrop={e => {
                      e.preventDefault();
                      e.currentTarget.dataset.dragover = 'false';
                      const files = Array.from(e.dataTransfer.files);
                      setBootstrapFiles(prev => [...prev, ...files]);
                    }}
                    onClick={() => {
                      const input = document.createElement('input');
                      input.type = 'file';
                      input.multiple = true;
                      input.accept = '.md,.txt,.pdf,.json,.yaml,.yml,.toml,.csv,.docx,.pptx';
                      input.onchange = () => {
                        if (input.files) setBootstrapFiles(prev => [...prev, ...Array.from(input.files!)]);
                      };
                      input.click();
                    }}
                  >
                    <FileText size={16} className="text-muted" />
                    <span className="text-sm text-muted">{t('projects.bootstrap.docsHint')}</span>
                  </div>
                  {bootstrapFiles.length > 0 && (
                    <div className="dash-file-list">
                      {bootstrapFiles.map((f, i) => (
                        <div key={i} className="dash-file-chip">
                          <FileText size={10} />
                          <span>{f.name}</span>
                          <button type="button" onClick={() => setBootstrapFiles(prev => prev.filter((_, j) => j !== i))} className="dash-file-remove">&times;</button>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                {/* Bootstrap++ options (MCP-aware) */}
                {(() => {
                  const gitMcps = mcpOverview.configs.filter(c =>
                    c.server_id === 'mcp-github' || c.server_id === 'mcp-gitlab'
                  );
                  const trackerMcps = mcpOverview.configs.filter(c =>
                    c.server_id === 'mcp-github' || c.server_id === 'mcp-atlassian' || c.server_id === 'mcp-linear'
                  );
                  if (!gitMcps.length && !trackerMcps.length) return null;
                  return (
                    <div className="dash-bootstrap-options" style={{ marginBottom: 16 }}>
                      <span className="dash-field-label">{t('projects.bootstrap.options')}</span>
                      {gitMcps.length > 0 && (
                        <div className="dash-bootstrap-option">
                          <select
                            className="dash-field-input"
                            value={bootstrapRepoMcp}
                            onChange={e => setBootstrapRepoMcp(e.target.value)}
                            style={{ flex: 1 }}
                          >
                            <option value="">{t('projects.bootstrap.noCreateRepo')}</option>
                            {gitMcps.map(c => (
                              <option key={c.id} value={c.id}>{t('projects.bootstrap.createRepoVia')} {c.label}</option>
                            ))}
                          </select>
                        </div>
                      )}
                      {trackerMcps.length > 0 && (
                        <div className="dash-bootstrap-option">
                          <select
                            className="dash-field-input"
                            value={bootstrapTrackerMcp}
                            onChange={e => setBootstrapTrackerMcp(e.target.value)}
                            style={{ flex: 1 }}
                          >
                            <option value="">{t('projects.bootstrap.noGeneratePlan')}</option>
                            {trackerMcps.map(c => (
                              <option key={c.id} value={c.id}>{t('projects.bootstrap.generatePlanVia')} {c.label}</option>
                            ))}
                          </select>
                        </div>
                      )}
                    </div>
                  );
                })()}

                <button
                  onClick={handleBootstrap}
                  disabled={bootstrapLoading || !bootstrapName.trim() || !bootstrapDesc.trim()}
                  className="dash-submit-btn"
                >
                  {bootstrapLoading ? <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /> : <Zap size={16} />}
                  {bootstrapLoading ? t('projects.bootstrap.creating') : t('projects.bootstrap.start')}
                </button>
              </div>
            )}
            {newProjectMode === 'clone' && (
              <div onKeyDown={e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && cloneUrl.trim() && !cloneLoading) handleClone(); }}>
                <label className="dash-field">
                  <span className="dash-field-label">{t('projects.clone.url')}</span>
                  <input
                    value={cloneUrl} onChange={e => setCloneUrl(e.target.value)}
                    className="dash-field-input"
                    placeholder={t('projects.clone.urlPlaceholder')}
                    autoFocus
                  />
                </label>
                <label className="dash-field" style={{ marginBottom: 16 }}>
                  <span className="dash-field-label">{t('projects.clone.name')}</span>
                  <input
                    value={cloneName} onChange={e => setCloneName(e.target.value)}
                    className="dash-field-input"
                    placeholder="my-project"
                  />
                </label>
                <div className="flex-row gap-4 mb-8">
                  <button
                    onClick={handleClone}
                    disabled={cloneLoading || !cloneUrl.trim()}
                    className="dash-submit-btn flex-1"
                  >
                    {cloneLoading ? <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /> : <Folder size={16} />}
                    {cloneLoading ? t('projects.clone.cloning') : t('projects.clone.start')}
                  </button>
                  <button
                    onClick={() => handleDiscoverRepos()}
                    disabled={discoverLoading}
                    className="dash-discover-btn"
                  >
                    {discoverLoading ? <Loader2 size={14} style={{ animation: 'spin 1s linear infinite' }} /> : <Search size={14} />}
                    {discoverLoading ? t('projects.clone.discovering') : t('projects.clone.discover')}
                  </button>
                </div>

                {availableSources.length > 1 && (
                  <div className="mb-6">
                    <div className="dash-source-label">{t('projects.clone.selectSources')}</div>
                    <div className="flex-wrap gap-3" style={{ display: 'flex' }}>
                      {availableSources.map(src => (
                        <button
                          key={src.id}
                          onClick={() => toggleSource(src.id)}
                          className="dash-source-chip"
                          data-active={selectedSourceIds.includes(src.id)}
                        >
                          {src.provider === 'github' ? '🐙' : '🦊'} {src.label}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {discoverError && (
                  <div className="dash-discover-error">
                    {discoverError}
                  </div>
                )}

                {discoverSources.length > 0 && (
                  <div className="dash-discover-summary">
                    {t('projects.clone.discovered').replace('{0}', String(discoveredRepos.length)).replace('{1}', discoverSources.join(', '))}
                  </div>
                )}

                {discoveredRepos.length > 0 && (
                  <div>
                    {discoveredRepos.length > 10 && (
                      <div className="mb-4">
                        <input
                          type="text"
                          value={repoSearch}
                          onChange={e => setRepoSearch(e.target.value)}
                          placeholder={t('projects.clone.searchRepos')}
                          className="dash-repo-search"
                        />
                      </div>
                    )}
                  <div className="dash-repo-list">
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
                        className="dash-repo-item"
                        style={{ opacity: repo.already_cloned ? 0.5 : 1 }}
                      >
                        <div className="flex-1 min-w-0">
                          <div className="dash-repo-name">
                            {repo.full_name}
                            {repo.language && (
                              <span className="dash-repo-lang">{repo.language}</span>
                            )}
                            {repo.stargazers_count > 0 && (
                              <span className="dash-repo-stars">&#9733; {repo.stargazers_count}</span>
                            )}
                          </div>
                          {repo.description && (
                            <div className="dash-repo-desc">
                              {repo.description}
                            </div>
                          )}
                        </div>
                        {repo.already_cloned ? (
                          <span className="dash-repo-cloned">
                            {t('projects.clone.alreadyCloned')}
                          </span>
                        ) : (
                          <button
                            onClick={() => handleCloneDiscovered(repo)}
                            disabled={cloneLoading}
                            className="dash-repo-clone-btn"
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
            {newProjectMode === 'folder' && (
              <div>
                <p className="text-sm text-muted mb-4">{t('projects.folder.hint')}</p>
                <label className="dash-field">
                  <span className="dash-field-label">{t('projects.folder.path')}</span>
                  <input
                    value={folderPath} onChange={e => setFolderPath(e.target.value)}
                    className="dash-field-input"
                    placeholder="/home/user/my-docs"
                    autoFocus
                  />
                </label>
                <label className="dash-field" style={{ marginBottom: 16 }}>
                  <span className="dash-field-label">{t('projects.folder.name')}</span>
                  <input
                    value={folderName} onChange={e => setFolderName(e.target.value)}
                    className="dash-field-input"
                    placeholder={t('projects.folder.namePlaceholder')}
                  />
                </label>
                <button
                  onClick={async () => {
                    if (!folderPath.trim()) return;
                    setFolderLoading(true);
                    try {
                      await projectsApi.addFolder({ path: folderPath.trim(), name: folderName.trim() || undefined });
                      toast(t('projects.folder.success'), 'success');
                      setShowBootstrap(false);
                      setFolderPath('');
                      setFolderName('');
                      refetch();
                    } catch (e) {
                      toast(userError(e), 'error');
                    } finally {
                      setFolderLoading(false);
                    }
                  }}
                  disabled={folderLoading || !folderPath.trim()}
                  className="dash-submit-btn"
                >
                  {folderLoading ? <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} /> : <FolderOpen size={16} />}
                  {folderLoading ? '...' : t('projects.folder.add')}
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Content */}
      <main className="dash-main">

        {/* ════════ PROJETS ════════ */}
        {page === 'projects' && (<ErrorBoundary mode="zone" label="Projects">
          {projectsLoading && (
            <div className="dash-loading-bar">
              <Loader2 size={14} className="spin" />
              <span className="text-sm text-muted">{t('projects.loading')}</span>
            </div>
          )}
          <ProjectList
            projects={projects}
            discussions={allDiscussions}
            discussionsByProject={discussionsByProject}
            driftByProject={driftByProject}
            agents={agents}
            allSkills={allSkills}
            mcpConfigs={mcpOverview.configs}
            workflows={workflowList ?? []}
            configLanguage={configLanguage ?? null}
            toast={toast}
            onNavigate={(p) => {
              if (p.startsWith('mcps:')) {
                setMcpSelectedConfigId(p.split(':')[1]);
                setPage('mcps');
              } else {
                setPage(p as Page);
              }
            }}
            onSetDiscPrefill={setDiscPrefill}
            onAutoRunDiscussion={setAutoRunDiscussionId}
            onOpenDiscussion={setOpenDiscussionId}
            onRefetch={refetch}
            onRefetchDiscussions={refetchDiscussions}
            onRefetchSkills={refetchSkills}
            onRefetchDrift={handleRefetchDrift}
            expandedId={expandedId}
            onSetExpandedId={setExpandedId}
          />
        </ErrorBoundary>)}

        {/* ════════ Plugins ════════ */}
        {page === 'mcps' && (
          <ErrorBoundary mode="zone" label="Plugins">
            <McpPage projects={projects} mcpOverview={mcpOverview} mcpRegistry={mcpRegistry} refetchMcps={refetchMcps} initialSelectedConfigId={mcpSelectedConfigId} />
          </ErrorBoundary>
        )}

        {/* ════════ WORKFLOWS ════════ */}
        {page === 'workflows' && (
          <ErrorBoundary mode="zone" label="Workflows">
            <WorkflowsPage
              projects={projects}
              installedAgentTypes={agents.filter(isUsable).map(a => a.agent_type)}
              agentAccess={agentAccess ?? undefined}
              configLanguage={configLanguage ?? undefined}
              toast={toast}
              initialSelectedWorkflowId={openWorkflowId}
              onInitialSelectionConsumed={() => setOpenWorkflowId(null)}
              onNavigateToBatch={(batchRunId) => {
                setFocusBatchId(batchRunId);
                setPage('discussions');
              }}
              onNavigateDiscussion={(discId) => { setAutoRunDiscussionId(discId); setPage('discussions'); }}
              onBatchLaunched={(discIds, batchRunId) => {
                // Mark every batch-child disc as sending so the sidebar
                // spinner lights up for all of them in parallel, not just
                // the one we navigate to. The parent (Dashboard) owns
                // sendingMap; WorkflowsPage only lives in the workflow tab.
                setSendingMap(prev => {
                  const next = { ...prev };
                  for (const id of discIds) next[id] = true;
                  return next;
                });
                setSendingStartMap(prev => {
                  const next = { ...prev };
                  const now = Date.now();
                  for (const id of discIds) next[id] = now;
                  return next;
                });
                // Navigate to the discussions tab and focus the batch
                // group in the sidebar — expand the project group + the
                // batch group + scroll to it. `focusBatchId` is consumed
                // by DiscussionsPage's useEffect which handles the expand
                // + scroll after the refetch settles.
                if (discIds.length > 0) {
                  setOpenDiscussionId(discIds[0]);
                  setFocusBatchId(batchRunId);
                  setPage('discussions');
                }
                // Force a refetch so the new discs show up in the sidebar
                // grouped under their batch run.
                refetchDiscussions?.();
              }}
            />
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
                setTimeout(() => {
                  document.getElementById(`project-${opts.projectId}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' });
                }, 100);
              }
              if (opts?.scrollTo) {
                setTimeout(() => {
                  document.getElementById(opts.scrollTo!)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
                }, 200);
              }
              // Sidebar batch pastille → workflows tab + pre-open the parent workflow's detail.
              if (opts?.workflowId) {
                setOpenWorkflowId(opts.workflowId);
              }
            }}
            prefill={discPrefill}
            onPrefillConsumed={handlePrefillConsumed}
            autoRunDiscussionId={autoRunDiscussionId}
            onAutoRunConsumed={handleAutoRunConsumed}
            openDiscussionId={openDiscussionId}
            onOpenDiscConsumed={handleOpenDiscConsumed}
            focusBatchId={focusBatchId}
            onFocusBatchConsumed={() => setFocusBatchId(null)}
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
            initialActiveDiscussionId={openDiscussionId ?? activeDiscussionId}
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
            onNavigateDiscussion={(id) => { setOpenDiscussionId(id); setPage('discussions'); }}
            toast={toast}
          />
          </ErrorBoundary>
        )}
      </main>
      <TourOverlay />
      </TourProvider>
    </div>
  );
}
