import '../pages/Dashboard.css';
import { useState, useMemo, useDeferredValue, useEffect, useRef, useCallback } from 'react';
import { useT } from '../lib/I18nContext';
import { getProjectGroup, isHiddenPath, isValidationDisc } from '../lib/constants';
import { useIsMobile } from '../hooks/useMediaQuery';
import { ProjectCard } from './ProjectCard';
import { ListControls } from './ListControls';
import type { Project, AgentDetection, AuditProgress, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary } from '../types/generated';
import {
  Folder, ChevronDown, ChevronRight, ChevronLeft, Search, X, AlertTriangle,
  MessageSquare, Workflow, Puzzle, ShieldCheck, Loader2, FileCode, Clock, BookOpen,
} from 'lucide-react';
import { MatrixText } from './MatrixText';

const isAiReady = (p: Project) => p.audit_status !== 'NoTemplate';
type ProjectFilter = 'visible' | 'attention' | 'validated' | 'missing' | 'hidden' | 'all';
type ProjectSort = 'name' | 'updated' | 'status' | 'techDebt';

const PROJECT_STATUS_RANK: Record<Project['audit_status'], number> = {
  NoTemplate: 0,
  TemplateInstalled: 1,
  Bootstrapped: 2,
  Audited: 3,
  Validated: 4,
};

export interface ProjectListProps {
  projects: Project[];
  /** Fleet-wide live audits (Dashboard poll) — lets each card adopt an
   *  audit launched outside the UI (MCP bridge, CLI). */
  activeAudits?: AuditProgress[];
  discussions: Discussion[];
  discussionsByProject: Record<string, Discussion[]>;
  driftByProject: Record<string, DriftCheckResponse>;
  agents: AgentDetection[];
  allSkills: Skill[];
  mcpConfigs: McpConfigDisplay[];
  workflows: WorkflowSummary[];
  configLanguage: string | null;
  toast: (msg: string, type: 'success' | 'error' | 'warning' | 'info') => void;
  onNavigate: (page: string) => void;
  onSetDiscPrefill: (prefill: { projectId: string; title: string; prompt: string; locked?: boolean }) => void;
  onAutoRunDiscussion: (discId: string) => void;
  onOpenDiscussion: (discId: string) => void;
  onRefetch: () => void;
  onRefetchDiscussions: () => void;
  onRefetchSkills: () => void;
  onRefetchDrift: (projectId: string) => void;
  expandedId: string | null;
  onSetExpandedId: (id: string | null) => void;
}

export function ProjectList({
  projects,
  activeAudits = [],
  discussionsByProject,
  driftByProject,
  agents,
  allSkills,
  mcpConfigs,
  workflows,
  configLanguage,
  toast,
  onNavigate,
  onSetDiscPrefill,
  onAutoRunDiscussion,
  onOpenDiscussion,
  onRefetch,
  onRefetchDiscussions,
  onRefetchSkills,
  onRefetchDrift,
  expandedId,
  onSetExpandedId,
}: ProjectListProps) {
  const { t } = useT();
  const isMobile = useIsMobile();

  const [projectSearch, setProjectSearch] = useState('');
  const [projectFilter, setProjectFilter] = useState<ProjectFilter>('visible');
  const [projectSort, setProjectSort] = useState<ProjectSort>('name');
  const [projectSortReversed, setProjectSortReversed] = useState(false);
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  // Sentinel ref attached to the "Show more" button. An IntersectionObserver
  // below auto-bumps the cap whenever the sentinel scrolls into view, so the
  // list feels infinite even though we only ever mount ~20 cards more at a
  // time (matches the "Show more" button bump). On 250+ project installs
  // this avoids the artificial scroll-stop at each 20-step plateau.
  const loadMoreRef = useRef<HTMLButtonElement | null>(null);
  const listPaneRef = useRef<HTMLDivElement | null>(null);
  const detailPaneRef = useRef<HTMLDivElement | null>(null);

  // Search input vs derived filter — useDeferredValue lets the keystroke
  // commit immediately on the input, while the heavy filter / sort /
  // grouping pipeline runs at lower priority on the deferred value. On a
  // 250-projects seed, before fix: 787 ms / +6867 DOM nodes per keystroke
  // (the cap=20 dropped to "show all matches" on any non-empty search,
  // mounting ~200 ProjectCard subtrees synchronously).
  const deferredSearch = useDeferredValue(projectSearch);
  const searchLower = deferredSearch.toLowerCase();

  const visibleProjects = useMemo(() => projects.filter(p => !isHiddenPath(p.path)), [projects]);
  const hiddenProjects = useMemo(() => projects.filter(p => isHiddenPath(p.path)), [projects]);
  // Projects whose directory no longer resolves on disk (e.g. after a
  // cross-OS DB import). `path_exists === false` is explicit — `undefined`
  // (legacy payload) is treated as present, never flagged.
  const missingPathProjects = useMemo(
    () => projects.filter(p => p.path_exists === false && !isHiddenPath(p.path)),
    [projects],
  );

  const filteredProjects = useMemo(() => {
    let list = projects;
    switch (projectFilter) {
      case 'visible':
        list = visibleProjects;
        break;
      case 'attention':
        list = visibleProjects.filter(p =>
          p.path_exists === false
          || p.audit_status !== 'Validated'
          || (p.tech_debt_count ?? 0) > 0
          || p.needs_docs_migration
          || (driftByProject[p.id]?.stale_sections.length ?? 0) > 0
        );
        break;
      case 'validated':
        list = visibleProjects.filter(p => p.audit_status === 'Validated');
        break;
      case 'missing':
        list = missingPathProjects;
        break;
      case 'hidden':
        list = hiddenProjects;
        break;
      case 'all':
        break;
    }
    if (deferredSearch) list = list.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower));
    return list;
  }, [
    projects,
    visibleProjects,
    hiddenProjects,
    missingPathProjects,
    projectFilter,
    deferredSearch,
    searchLower,
    driftByProject,
  ]);

  const projGroup = useCallback(
    (p: Project) => getProjectGroup(p, t('projects.group.local'), t('projects.group.other')),
    [t],
  );

  const sortedProjects = useMemo(() => [...filteredProjects].sort((a, b) => {
    let result: number;
    if (projectSort === 'name') {
      const groupA = projGroup(a);
      const groupB = projGroup(b);
      result = groupA === groupB ? a.name.localeCompare(b.name) : groupA.localeCompare(groupB);
    } else if (projectSort === 'updated') {
      result = new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
    } else if (projectSort === 'status') {
      result = PROJECT_STATUS_RANK[a.audit_status] - PROJECT_STATUS_RANK[b.audit_status];
    } else {
      result = (b.tech_debt_count ?? 0) - (a.tech_debt_count ?? 0);
    }
    if (result === 0) result = a.name.localeCompare(b.name);
    return projectSortReversed ? -result : result;
  }), [filteredProjects, projectSort, projectSortReversed, projGroup]);

  const groupedProjects = useMemo(() => {
    const groups: { group: string; projects: Project[] }[] = [];
    for (const p of sortedProjects) {
      const group = projGroup(p);
      const last = groups[groups.length - 1];
      if (last && last.group === group) { last.projects.push(p); }
      else { groups.push({ group, projects: [p] }); }
    }
    return groups;
  }, [sortedProjects, projGroup]);

  // Infinite-scroll wiring — observe the "Show more" sentinel and bump the
  // cap when it enters the viewport. `rootMargin: 200px` means we start
  // loading the next batch a bit before the user actually reaches the
  // bottom, so the scroll stays continuous instead of stuttering at the
  // plateau. We only attach when there's still more to load (the button
  // itself is conditional on `remainingCount > 0`).
  useEffect(() => {
    const el = loadMoreRef.current;
    if (!el) return;
    const observer = new IntersectionObserver(entries => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          setProjectDisplayLimit(prev => prev + 20);
        }
      }
    }, { root: listPaneRef.current, rootMargin: '200px' });
    observer.observe(el);
    return () => { observer.disconnect(); };
    // We re-attach whenever the cap changes (button re-mounts) so the
    // observer always tracks the *current* sentinel.
  }, [projectDisplayLimit]);

  // KEY UX-perf change: the cap stays in effect even when searching.
  // Pre-fix `projectSearch ? sortedProjects : ...` mounted every match
  // (200+ cards) on the first keystroke. Now we cap at the same limit
  // and surface a "Show more" CTA — same as the no-search case. The
  // typical user finds their project in the first 20 matches anyway.
  const displayProjects = useMemo(
    () => sortedProjects.slice(0, projectDisplayLimit),
    [sortedProjects, projectDisplayLimit],
  );
  const remainingCount = sortedProjects.length - displayProjects.length;
  const aiCount = visibleProjects.filter(isAiReady).length;
  const expandedProject = expandedId
    ? filteredProjects.find(project => project.id === expandedId) ?? null
    : null;
  const selectedProject = expandedProject ?? (!isMobile ? displayProjects[0] ?? null : null);

  useEffect(() => {
    if (!isMobile && selectedProject && selectedProject.id !== expandedId) {
      onSetExpandedId(selectedProject.id);
    }
  }, [expandedId, isMobile, onSetExpandedId, selectedProject]);

  const selectProject = (projectId: string) => {
    onSetExpandedId(projectId);
    requestAnimationFrame(() => {
      detailPaneRef.current?.scrollTo({ top: 0, behavior: 'smooth' });
    });
  };

  return (
    <div className="project-page">
      <div className="dash-page-header project-page-header">
        <div>
          <h1 className="dash-h1"><MatrixText text={t('projects.title')} /></h1>
          <p className="dash-meta">
            {aiCount}/{visibleProjects.length} {t('projects.aiReady')}
            {hiddenProjects.length > 0 && (
              <span className="text-faint"> + {hiddenProjects.length} {hiddenProjects.length > 1 ? t('projects.hiddenPlural') : t('projects.hidden')}</span>
            )}
          </p>
        </div>
      </div>

      {missingPathProjects.length > 0 && (
        <div className="dash-missing-banner" role="status" data-testid="missing-path-banner">
          <AlertTriangle size={15} className="dash-missing-banner-icon" />
          <span className="dash-missing-banner-text">
            {missingPathProjects.length > 1
              ? t('projects.missingBanner.plural', missingPathProjects.length)
              : t('projects.missingBanner.one')}
          </span>
          <button
            className="dash-missing-banner-btn"
            onClick={() => setProjectFilter(current => current === 'missing' ? 'visible' : 'missing')}
            aria-pressed={projectFilter === 'missing'}
          >
            {projectFilter === 'missing' ? t('projects.missingBanner.showAll') : t('projects.missingBanner.showOnly')}
          </button>
        </div>
      )}

      <div className="project-list-toolbar">
        <div className="dash-search-wrap project-search-wrap">
          <Search size={14} className="dash-search-icon" />
          <input
            className="dash-search-input"
            placeholder={t('projects.search')}
            value={projectSearch}
            onChange={(event) => setProjectSearch(event.target.value)}
          />
          {projectSearch && (
            <button
              className="dash-search-clear"
              onClick={() => setProjectSearch('')}
              aria-label={t('projects.master.clear')}
            >
              <X size={12} />
            </button>
          )}
        </div>
        <ListControls
          filterLabel={t('projects.master.filter')}
          filterValue={projectFilter}
          filterOptions={[
            { value: 'visible', label: t('projects.master.filter.visible') },
            { value: 'attention', label: t('projects.master.filter.attention') },
            { value: 'validated', label: t('projects.master.filter.validated') },
            { value: 'missing', label: t('projects.master.filter.missing'), disabled: missingPathProjects.length === 0 },
            { value: 'hidden', label: t('projects.master.filter.hidden'), disabled: hiddenProjects.length === 0 },
            { value: 'all', label: t('projects.master.filter.all') },
          ]}
          onFilterChange={setProjectFilter}
          sortLabel={t('projects.master.sort')}
          sortValue={projectSort}
          sortOptions={[
            { value: 'name', label: t('projects.master.sort.name') },
            { value: 'updated', label: t('projects.master.sort.updated') },
            { value: 'status', label: t('projects.master.sort.status') },
            { value: 'techDebt', label: t('projects.master.sort.techDebt') },
          ]}
          onSortChange={setProjectSort}
          reversed={projectSortReversed}
          onToggleDirection={() => setProjectSortReversed(value => !value)}
          directionLabel={t('projects.master.sort.direction')}
        />
      </div>

      <div className="project-workspace" data-mobile={isMobile}>
        {!(isMobile && selectedProject) && (
          <div ref={listPaneRef} className="project-list-pane" data-testid="project-list-pane">
            <div className="project-list-result-count">
              {t('projects.master.results', sortedProjects.length)}
            </div>
            {displayProjects.map((proj: Project, idx: number) => {
              const isSelected = selectedProject?.id === proj.id;
              const projHidden = isHiddenPath(proj.path);
              const currentGroup = projGroup(proj);
              const prevGroup = idx > 0 ? projGroup(displayProjects[idx - 1]) : null;
              const showGroupHeader = projectSort === 'name'
                && projectFilter === 'visible'
                && !projectSearch
                && groupedProjects.length > 1
                && currentGroup !== prevGroup;
              const groupColor = currentGroup === t('projects.group.local')
                ? 'var(--kr-text-dim)'
                : `hsl(${Math.abs([...currentGroup].reduce((h, c) => h * 31 + c.charCodeAt(0), 0)) % 360}, 70%, 38%)`;
              const groupProjectCount = groupedProjects.find(group => group.group === currentGroup)?.projects.length ?? 0;
              const projDiscussions = discussionsByProject[proj.id] ?? [];
              const liveAudit = activeAudits.some(audit => audit.project_id === proj.id);
              const validating = proj.audit_status === 'Audited'
                && projDiscussions.some(discussion => isValidationDisc(discussion.title) && !discussion.archived);
              const projectMcpCount = mcpConfigs.filter(config => config.is_global || config.project_ids.includes(proj.id)).length;
              const projectWorkflowCount = workflows.filter(workflow => workflow.project_id === proj.id).length;
              const staleCount = driftByProject[proj.id]?.stale_sections.length ?? 0;
              const status = liveAudit
                ? { label: t('projects.master.status.auditRunning'), tone: 'running', icon: <Loader2 size={10} className="spin" /> }
                : validating
                  ? { label: t('projects.status.validating'), tone: 'running', icon: <Loader2 size={10} className="spin" /> }
                  : proj.audit_status === 'Validated'
                    ? { label: t('projects.master.status.validated'), tone: 'success', icon: <ShieldCheck size={10} /> }
                    : proj.audit_status === 'Audited'
                      ? { label: t('projects.master.status.toValidate'), tone: 'warning', icon: <ShieldCheck size={10} /> }
                      : proj.audit_status === 'Bootstrapped'
                        ? { label: t('projects.status.bootstrapped'), tone: 'info', icon: <FileCode size={10} /> }
                        : { label: t('projects.master.status.toPrepare'), tone: 'muted', icon: <FileCode size={10} /> };

              return (
                <div key={proj.id}>
                  {showGroupHeader && (() => {
                    const isCollapsed = collapsedGroups.has(currentGroup);
                    return (
                      <button
                        className="dash-group-btn project-group-btn"
                        data-first={idx === 0}
                        onClick={() => setCollapsedGroups(previous => {
                          const next = new Set(previous);
                          if (next.has(currentGroup)) next.delete(currentGroup); else next.add(currentGroup);
                          return next;
                        })}
                        aria-expanded={!isCollapsed}
                      >
                        <ChevronDown
                          size={13}
                          style={{
                            color: groupColor,
                            transform: isCollapsed ? 'rotate(-90deg)' : 'none',
                            transition: 'transform 0.15s',
                            flexShrink: 0,
                          }}
                        />
                        <span className="dash-group-bar" style={{ background: groupColor }} />
                        <span className="dash-group-label" style={{ color: groupColor }}>{currentGroup}</span>
                        <span className="dash-group-count">({groupProjectCount})</span>
                        <span className="dash-group-line" style={{ background: `${groupColor}20` }} />
                      </button>
                    );
                  })()}
                  {!collapsedGroups.has(currentGroup) && (
                    <button
                      id={`project-${proj.id}`}
                      type="button"
                      className="project-list-card"
                      data-active={isSelected}
                      data-hidden={projHidden}
                      data-testid={`project-list-item-${proj.id}`}
                      onClick={() => selectProject(proj.id)}
                      aria-pressed={isSelected}
                    >
                      <span className="project-list-card-rail" data-tone={status.tone} />
                      <span className="project-list-card-head">
                        <span className="project-list-card-name">{proj.name}</span>
                        <ChevronRight size={13} className="project-list-card-chevron" />
                      </span>
                      <span className="project-list-card-path" title={proj.path}>{proj.path}</span>
                      <span className="project-list-card-status">
                        <span className="project-status-chip" data-tone={status.tone}>
                          {status.icon}
                          {status.label}
                        </span>
                        {proj.path_exists === false && (
                          <span className="project-alert-chip" data-tone="error">
                            <AlertTriangle size={9} /> {t('projects.master.pathMissing')}
                          </span>
                        )}
                        {(proj.tech_debt_count ?? 0) > 0 && (
                          <span className="project-alert-chip" data-tone="warning">
                            <AlertTriangle size={9} /> {proj.tech_debt_count} TD
                          </span>
                        )}
                        {(proj.onboarding_count ?? 0) > 0 && (
                          <span className="project-alert-chip" data-tone="info">
                            <BookOpen size={9} /> {t('projects.master.onboarding', proj.onboarding_count)}
                          </span>
                        )}
                        {staleCount > 0 && (
                          <span className="project-alert-chip" data-tone="warning">
                            <Clock size={9} /> {t('projects.master.stale', staleCount)}
                          </span>
                        )}
                      </span>
                      <span className="project-list-card-meta">
                        <span><MessageSquare size={11} /> {projDiscussions.length}</span>
                        <span><Puzzle size={11} /> {projectMcpCount}</span>
                        <span><Workflow size={11} /> {projectWorkflowCount}</span>
                      </span>
                    </button>
                  )}
                </div>
              );
            })}

            {remainingCount > 0 && (
              <button
                ref={loadMoreRef}
                className="dash-show-more-btn"
                onClick={() => setProjectDisplayLimit(previous => previous + 20)}
              >
                {t('projects.showMore', remainingCount, remainingCount > 1 ? 's' : '', remainingCount > 1 ? 's' : '')}
              </button>
            )}
            {!projectSearch && projectDisplayLimit > 20 && remainingCount === 0 && sortedProjects.length > 20 && (
              <button className="dash-collapse-btn" onClick={() => setProjectDisplayLimit(20)}>
                {t('projects.collapse')}
              </button>
            )}
            {displayProjects.length === 0 && (
              <div className="dash-empty project-list-empty">
                <Folder size={30} />
                <p className="dash-empty-text">
                  {projectSearch ? t('projects.emptySearch') : t('projects.emptyHint')}
                </p>
              </div>
            )}
          </div>
        )}

        {(!isMobile || selectedProject) && (
          <div ref={detailPaneRef} className="project-detail-pane" data-testid="project-detail-pane">
            {isMobile && selectedProject && (
              <button className="project-back-btn" onClick={() => onSetExpandedId(null)}>
                <ChevronLeft size={14} /> {t('projects.master.back')}
              </button>
            )}
            {selectedProject ? (
              <ProjectCard
                key={selectedProject.id}
                detailMode
                project={selectedProject}
                externalAuditLive={activeAudits.some(audit => audit.project_id === selectedProject.id)}
                isOpen
                onToggleOpen={() => {}}
                discussions={discussionsByProject[selectedProject.id] ?? []}
                driftStatus={driftByProject[selectedProject.id]}
                agents={agents}
                allSkills={allSkills}
                mcpConfigs={mcpConfigs}
                workflows={workflows}
                configLanguage={configLanguage}
                toast={toast}
                onNavigate={onNavigate}
                onSetDiscPrefill={onSetDiscPrefill}
                onAutoRunDiscussion={onAutoRunDiscussion}
                onOpenDiscussion={onOpenDiscussion}
                onRefetch={onRefetch}
                onRefetchDiscussions={onRefetchDiscussions}
                onRefetchSkills={onRefetchSkills}
                onRefetchDrift={onRefetchDrift}
              />
            ) : (
              <div className="project-detail-empty">
                <Folder size={32} />
                <p>{t('projects.master.select')}</p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
