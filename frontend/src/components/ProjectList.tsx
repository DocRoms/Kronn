import '../pages/Dashboard.css';
import { useState, useMemo, useDeferredValue, useEffect, useRef, useCallback } from 'react';
import { useT } from '../lib/I18nContext';
import { getProjectGroup, isHiddenPath, isValidationDisc } from '../lib/constants';
import { useIsMobile } from '../hooks/useMediaQuery';
import { ProjectCard } from './ProjectCard';
import { ListControls } from './ListControls';
import { CollectionShell, type CollectionFilter } from './CollectionShell';
import { projects as projectsApi } from '../lib/api';
import { usePersistentIdSet } from '../hooks/usePersistentIdSet';
import type { Project, AgentDetection, AuditProgress, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary } from '../types/generated';
import {
  Folder, ChevronRight, AlertTriangle,
  MessageSquare, Workflow, Puzzle, ShieldCheck, Loader2, FileCode, Clock, Plus,
  ArrowUpDown, Filter, Star, Trash2,
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
  onAddProject?: () => void;
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
  onAddProject,
  expandedId,
  onSetExpandedId,
}: ProjectListProps) {
  const { t } = useT();
  const isMobile = useIsMobile();

  const [projectSearch, setProjectSearch] = useState('');
  const [projectFilter, setProjectFilter] = useState<ProjectFilter>('visible');
  const [projectSort, setProjectSort] = useState<ProjectSort>('name');
  const [projectSortReversed, setProjectSortReversed] = useState(false);
  const [filterOptionsOpen, setFilterOptionsOpen] = useState(false);
  const [sortOptionsOpen, setSortOptionsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const projectIds = useMemo(() => projects.map(project => project.id), [projects]);
  const { ids: favoriteIds, toggle: toggleFavorite } = usePersistentIdSet('kronn:collection-favorites:projects', projectIds);
  const shellRootRef = useRef<HTMLDivElement | null>(null);

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

  // Shared collection-shell filters — a single source of truth handed both
  // to CollectionShell (renders them as toggle buttons) and to the local
  // `filteredProjects` computation below (auto-select + result count),
  // since the shell doesn't expose its internally-filtered set.
  const projectFilters = useMemo<CollectionFilter<Project>[]>(() => {
    const all: CollectionFilter<Project>[] = [
      { id: 'visible', label: t('projects.master.filter.visible'), matches: p => !isHiddenPath(p.path) },
      {
        id: 'attention', label: t('projects.master.filter.attention'), matches: p =>
          !isHiddenPath(p.path) && (
            p.path_exists === false
            || p.audit_status !== 'Validated'
            || (p.tech_debt_count ?? 0) > 0
            || p.needs_docs_migration
            || (driftByProject[p.id]?.stale_sections.length ?? 0) > 0
          ),
      },
      { id: 'validated', label: t('projects.master.filter.validated'), matches: p => !isHiddenPath(p.path) && p.audit_status === 'Validated' },
      { id: 'missing', label: t('projects.master.filter.missing'), matches: p => p.path_exists === false && !isHiddenPath(p.path) },
      { id: 'hidden', label: t('projects.master.filter.hidden'), matches: p => isHiddenPath(p.path) },
      { id: 'all', label: t('projects.master.filter.all'), matches: () => true },
    ];
    return all.filter(filter =>
      (filter.id !== 'missing' || missingPathProjects.length > 0)
      && (filter.id !== 'hidden' || hiddenProjects.length > 0)
    );
  }, [t, driftByProject, missingPathProjects.length, hiddenProjects.length]);

  const filteredProjects = useMemo(() => {
    const activeFilter = projectFilters.find(filter => filter.id === projectFilter);
    let list = activeFilter ? projects.filter(activeFilter.matches) : projects;
    if (deferredSearch) list = list.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower));
    return list;
  }, [projects, projectFilters, projectFilter, deferredSearch, searchLower]);

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

  const aiCount = visibleProjects.filter(isAiReady).length;
  const selectedProject = expandedId
    ? filteredProjects.find(project => project.id === expandedId) ?? (!isMobile ? sortedProjects[0] ?? null : null)
    : (!isMobile ? sortedProjects[0] ?? null : null);

  useEffect(() => {
    if (!isMobile && selectedProject && selectedProject.id !== expandedId) {
      onSetExpandedId(selectedProject.id);
    }
  }, [expandedId, isMobile, onSetExpandedId, selectedProject]);

  const selectProject = (projectId: string) => {
    onSetExpandedId(projectId);
    requestAnimationFrame(() => {
      shellRootRef.current?.querySelector('.collection-shell-detail')?.scrollTo({ top: 0, behavior: 'smooth' });
    });
  };

  const deleteSelectedProjects = async (selected: Project[]) => {
    if (selected.length === 0 || !confirm(t('collection.deleteConfirm', selected.length))) return;
    try {
      await Promise.all(selected.map(project => projectsApi.delete(project.id)));
      if (expandedId && selected.some(project => project.id === expandedId)) onSetExpandedId(null);
      onRefetch();
      toast(t('collection.deleteSuccess', selected.length), 'success');
    } catch (cause) {
      toast(t('collection.deleteError', String(cause)), 'error');
      throw cause;
    }
  };

  return (
    <div className="project-page">
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

      <div ref={shellRootRef} className="project-shell">
        <CollectionShell<Project>
          ariaLabel={t('projects.title')}
          title={<MatrixText text={t('projects.title')} />}
          titleCount={visibleProjects.length}
          items={sortedProjects}
          getId={project => project.id}
          getLabel={project => `${project.name} ${project.path}`}
          isFavorite={project => favoriteIds.has(project.id)}
          onToggleFavorite={project => toggleFavorite(project.id)}
          filters={projectFilters}
          persistence={{
            query: projectSearch,
            onQueryChange: setProjectSearch,
            activeFilterId: projectFilter,
            onActiveFilterIdChange: id => setProjectFilter((id ?? 'all') as ProjectFilter),
            favoritesOnly,
            onFavoritesOnlyChange: setFavoritesOnly,
          }}
          selectedId={selectedProject?.id ?? null}
          onSelect={selectProject}
          selectedIds={selectedIds}
          onSelectedIdsChange={setSelectedIds}
          headerActions={onAddProject && <button
            type="button"
            className="collection-shell-icon collection-shell-primary-action"
            data-tour-id="new-project-btn"
            onClick={onAddProject}
            aria-label={t('projects.bootstrap')}
            title={t('projects.bootstrap')}
          >
            <Plus size={16} />
          </button>}
          actions={[{
            id: 'delete',
            label: t('collection.deleteSelected'),
            icon: <Trash2 size={15} />,
            danger: true,
            disabled: selected => selected.length === 0,
            onSelect: deleteSelectedProjects,
          }]}
          isMobile={isMobile}
          sidebarOpen={sidebarOpen}
          onSidebarOpenChange={setSidebarOpen}
          globalSearchShortcut
          showSearchClear
          showControls={false}
          labels={{
            search: t('projects.search'),
            favorites: t('collection.favorites'),
            clearFilters: t('collection.clearFilters'),
            moreActions: t('collection.moreActions'),
            openCollection: t('collection.openCollection'),
            closeCollection: t('collection.closeCollection'),
            selectItem: t('collection.selectItem'),
            selectMultiple: t('collection.selectMultiple'),
            cancelSelection: t('collection.cancelSelection'),
            selectedCount: count => t('collection.selectedCount', count),
          }}
          slots={{
            sidebarHeaderEnd: <>
              <button
                type="button"
                className="collection-shell-search-action collection-shell-search-action-icon"
                data-active={filterOptionsOpen || favoritesOnly || projectFilter !== 'visible'}
                onClick={() => {
                  setFilterOptionsOpen(open => !open);
                  setSortOptionsOpen(false);
                }}
                aria-expanded={filterOptionsOpen}
                aria-controls={filterOptionsOpen ? 'project-filter-options' : undefined}
                aria-label={t('projects.master.filter')}
                title={t('projects.master.filter')}
              >
                <Filter size={14} aria-hidden="true" />
              </button>
              <button
                type="button"
                className="collection-shell-search-action collection-shell-search-action-icon"
                data-active={sortOptionsOpen || projectSort !== 'name' || projectSortReversed}
                onClick={() => {
                  setSortOptionsOpen(open => !open);
                  setFilterOptionsOpen(false);
                }}
                aria-expanded={sortOptionsOpen}
                aria-controls={sortOptionsOpen ? 'project-sort-options' : undefined}
                aria-label={t('projects.master.sort')}
                title={t('projects.master.sort')}
              >
                <ArrowUpDown size={14} aria-hidden="true" />
              </button>
            </>,
            afterSidebarHeader: <>
              {filterOptionsOpen && <div id="project-filter-options" className="collection-shell-search-options collection-shell-controls">
                <button type="button" className="collection-shell-filter" data-active={favoritesOnly} aria-pressed={favoritesOnly} onClick={() => setFavoritesOnly(value => !value)}><Star size={14} />{t('collection.favorites')}</button>
                {projectFilters.map(filter => <button key={filter.id} type="button" className="collection-shell-filter" data-active={filter.id === projectFilter} aria-pressed={filter.id === projectFilter} onClick={() => setProjectFilter(filter.id === projectFilter ? 'all' : filter.id as ProjectFilter)}>{filter.label}</button>)}
                {(favoritesOnly || projectFilter !== 'all') && <button type="button" className="collection-shell-clear" onClick={() => { setFavoritesOnly(false); setProjectFilter('all'); }}>{t('collection.clearFilters')}</button>}
              </div>}
              {sortOptionsOpen && <div id="project-sort-options" className="collection-shell-search-options">
                <ListControls
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
              </div>}
              <div className="project-list-toolbar">
                <span className="dash-meta">
                  {aiCount}/{visibleProjects.length} {t('projects.aiReady')}
                  {hiddenProjects.length > 0 && <> + {hiddenProjects.length} {hiddenProjects.length > 1 ? t('projects.hiddenPlural') : t('projects.hidden')}</>}
                </span>
              </div>
            </>,
            renderDetail: () => (
              selectedProject ? (
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
              )
            ),
            renderEmpty: () => (
              <div className="project-list-empty">
                <Folder size={30} />
                <p className="dash-empty-text">
                  {projectSearch ? t('projects.emptySearch') : t('projects.emptyHint')}
                </p>
              </div>
            ),
            renderItem: (proj, { selected: isRowSelected }) => {
              const projHidden = isHiddenPath(proj.path);
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
                <div
                  id={`project-${proj.id}`}
                  className="project-list-card"
                  data-active={isRowSelected}
                  data-hidden={projHidden}
                  data-testid={`project-list-item-${proj.id}`}
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
                </div>
              );
            },
          }}
        />
      </div>
    </div>
  );
}
