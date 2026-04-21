import '../pages/Dashboard.css';
import { useState, useMemo } from 'react';
import { useT } from '../lib/I18nContext';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { ProjectCard } from './ProjectCard';
import type { Project, AgentDetection, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary } from '../types/generated';
import {
  Folder, ChevronDown, Eye, Search, X,
} from 'lucide-react';
import { MatrixText } from './MatrixText';

const isAiReady = (p: Project) => p.audit_status !== 'NoTemplate';

export interface ProjectListProps {
  projects: Project[];
  discussions: Discussion[];
  discussionsByProject: Record<string, Discussion[]>;
  driftByProject: Record<string, DriftCheckResponse>;
  agents: AgentDetection[];
  allSkills: Skill[];
  mcpConfigs: McpConfigDisplay[];
  workflows: WorkflowSummary[];
  configLanguage: string | null;
  toast: (msg: string, type: 'success' | 'error' | 'info') => void;
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

  const [showHidden, setShowHidden] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());

  const visibleProjects = useMemo(() => projects.filter(p => !isHiddenPath(p.path)), [projects]);
  const hiddenProjects = useMemo(() => projects.filter(p => isHiddenPath(p.path)), [projects]);
  const baseProjects = showHidden ? projects : visibleProjects;

  const searchLower = projectSearch.toLowerCase();
  const filteredProjects = projectSearch
    ? baseProjects.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower))
    : baseProjects;

  const projGroup = (p: Project) => getProjectGroup(p, t('projects.group.local'), t('projects.group.other'));

  const sortedProjects = useMemo(() => [...filteredProjects].sort((a, b) => {
    const groupA = projGroup(a);
    const groupB = projGroup(b);
    if (groupA !== groupB) return groupA.localeCompare(groupB);
    return a.name.localeCompare(b.name);
  }), [filteredProjects]);

  const groupedProjects = useMemo(() => {
    const groups: { group: string; projects: Project[] }[] = [];
    for (const p of sortedProjects) {
      const group = projGroup(p);
      const last = groups[groups.length - 1];
      if (last && last.group === group) { last.projects.push(p); }
      else { groups.push({ group, projects: [p] }); }
    }
    return groups;
  }, [sortedProjects]);

  const displayProjects = projectSearch ? sortedProjects : sortedProjects.slice(0, projectDisplayLimit);
  const remainingCount = sortedProjects.length - displayProjects.length;
  const aiCount = visibleProjects.filter(isAiReady).length;

  return (
    <div>
      <div className="dash-page-header">
        <div>
          <h1 className="dash-h1"><MatrixText text={t('projects.title')} /></h1>
          <p className="dash-meta">
            {aiCount}/{visibleProjects.length} {t('projects.aiReady')}
            {hiddenProjects.length > 0 && (
              <span className="text-faint"> + {hiddenProjects.length} {hiddenProjects.length > 1 ? t('projects.hiddenPlural') : t('projects.hidden')}</span>
            )}
          </p>
        </div>
        <div className="flex-row gap-4">
          {hiddenProjects.length > 0 && (
            <button className="dash-icon-btn" onClick={() => setShowHidden(!showHidden)} title={showHidden ? t('projects.hideHidden') : t('projects.showHidden')} aria-label={showHidden ? t('projects.hideHidden') : t('projects.showHidden')}>
              <Eye size={14} style={{ color: showHidden ? 'var(--kr-accent-ink)' : undefined }} />
            </button>
          )}
        </div>
      </div>

      {/* Search bar for projects */}
      {baseProjects.length > 3 && (
        <div className="dash-search-wrap">
          <Search size={14} className="dash-search-icon" />
          <input
            className="dash-search-input"
            placeholder={t('projects.search')}
            value={projectSearch}
            onChange={(e) => setProjectSearch(e.target.value)}
          />
          {projectSearch && (
            <button
              className="dash-search-clear"
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
        const currentGroup = projGroup(proj);
        const prevGroup = idx > 0 ? projGroup(displayProjects[idx - 1]) : null;
        const showGroupHeader = !projectSearch && groupedProjects.length > 1 && currentGroup !== prevGroup;
        const groupColor = currentGroup === t('projects.group.local') ? 'var(--kr-text-dim)' : `hsl(${Math.abs([...currentGroup].reduce((h, c) => h * 31 + c.charCodeAt(0), 0)) % 360}, 60%, 65%)`;
        const groupProjectCount = groupedProjects.find(g => g.group === currentGroup)?.projects.length ?? 0;
        const projDiscussions = discussionsByProject[proj.id] ?? [];

        return (
          <div key={proj.id}>
            {showGroupHeader && (() => {
              const isCollapsed = collapsedGroups.has(currentGroup);
              return (
                <button
                  className="dash-group-btn"
                  data-first={idx === 0}
                  onClick={() => setCollapsedGroups(prev => {
                    const next = new Set(prev);
                    if (next.has(currentGroup)) next.delete(currentGroup); else next.add(currentGroup);
                    return next;
                  })}
                  aria-expanded={!isCollapsed}
                >
                  <ChevronDown size={14} style={{ color: groupColor, transform: isCollapsed ? 'rotate(-90deg)' : 'none', transition: 'transform 0.15s', flexShrink: 0 }} />
                  <div className="dash-group-bar" style={{ background: groupColor }} />
                  <span className="dash-group-label" style={{ color: groupColor }}>
                    {currentGroup}
                  </span>
                  <span className="dash-group-count">
                    ({groupProjectCount})
                  </span>
                  <div className="dash-group-line" style={{ background: `${groupColor}20` }} />
                </button>
              );
            })()}
            {collapsedGroups.has(currentGroup) ? null : (
              <div style={{ opacity: projHidden ? 0.5 : 1 }}>
                <ProjectCard
                  project={proj}
                  isOpen={isOpen}
                  onToggleOpen={() => onSetExpandedId(isOpen ? null : proj.id)}
                  discussions={projDiscussions}
                  driftStatus={driftByProject[proj.id]}
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
              </div>
            )}
          </div>
        );
      })}

      {/* Show more / less buttons */}
      {remainingCount > 0 && (
        <button
          className="dash-show-more-btn"
          onClick={() => setProjectDisplayLimit(prev => prev + 20)}
        >
          {t('projects.showMore', remainingCount, remainingCount > 1 ? 's' : '', remainingCount > 1 ? 's' : '')}
        </button>
      )}
      {!projectSearch && projectDisplayLimit > 20 && remainingCount === 0 && sortedProjects.length > 20 && (
        <button
          className="dash-collapse-btn"
          onClick={() => setProjectDisplayLimit(20)}
        >
          {t('projects.collapse')}
        </button>
      )}

      {displayProjects.length === 0 && (
        <div className="dash-card dash-empty">
          <Folder size={32} style={{ color: 'var(--kr-text-ghost)', marginBottom: 12 }} />
          <p className="dash-empty-text">
            {projectSearch ? t('projects.emptySearch') : t('projects.emptyHint')}
          </p>
        </div>
      )}
    </div>
  );
}
