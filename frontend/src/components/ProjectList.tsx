import '../pages/Dashboard.css';
import { useState, useMemo, useDeferredValue, useEffect, useRef } from 'react';
import { useT } from '../lib/I18nContext';
import { getProjectGroup, isHiddenPath } from '../lib/constants';
import { ProjectCard } from './ProjectCard';
import type { Project, AgentDetection, DriftCheckResponse, Discussion, Skill, McpConfigDisplay, WorkflowSummary } from '../types/generated';
import {
  Folder, ChevronDown, Eye, Search, X, AlertTriangle,
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
  const [showOnlyMissing, setShowOnlyMissing] = useState(false);
  const [projectSearch, setProjectSearch] = useState('');
  const [projectDisplayLimit, setProjectDisplayLimit] = useState(20);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  // Sentinel ref attached to the "Show more" button. An IntersectionObserver
  // below auto-bumps the cap whenever the sentinel scrolls into view, so the
  // list feels infinite even though we only ever mount ~20 cards more at a
  // time (matches the "Show more" button bump). On 250+ project installs
  // this avoids the artificial scroll-stop at each 20-step plateau.
  const loadMoreRef = useRef<HTMLButtonElement | null>(null);

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
  const baseProjects = showHidden ? projects : visibleProjects;

  const filteredProjects = useMemo(() => {
    let list = baseProjects;
    if (showOnlyMissing) list = list.filter(p => p.path_exists === false);
    if (deferredSearch) list = list.filter(p => p.name.toLowerCase().includes(searchLower) || p.path.toLowerCase().includes(searchLower));
    return list;
  }, [baseProjects, showOnlyMissing, deferredSearch, searchLower]);

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
    }, { rootMargin: '200px' });
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
  const displayProjects = sortedProjects.slice(0, projectDisplayLimit);
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

      {/* Missing-path banner — persistent (unlike the import toast) so the
          operator can act on it any time after a cross-OS import. Offers a
          one-click filter down to just the projects that need remapping. */}
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
            onClick={() => setShowOnlyMissing(v => !v)}
            aria-pressed={showOnlyMissing}
          >
            {showOnlyMissing ? t('projects.missingBanner.showAll') : t('projects.missingBanner.showOnly')}
          </button>
        </div>
      )}

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
        // 2026-05-11: lightness from 65% → 38% so the label clears the
        // WCAG 4.5:1 threshold against light-theme cards (#f6f7f9).
        // Saturation bumped 60→70% to keep groups distinguishable at
        // the darker shade. Dark theme keeps the legacy mid-tone via
        // CSS-vars in the future if it ever feels too muted — for now
        // the same value passes there too.
        const groupColor = currentGroup === t('projects.group.local') ? 'var(--kr-text-dim)' : `hsl(${Math.abs([...currentGroup].reduce((h, c) => h * 31 + c.charCodeAt(0), 0)) % 360}, 70%, 38%)`;
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

      {/* Show more / less buttons.
       *  The button is also wired to an IntersectionObserver below so the
       *  next batch loads automatically as the user scrolls near the
       *  bottom — pre-fix, on a 250-project install the user could only
       *  see project 20 / 40 / 60 / 100… one click at a time and scroll
       *  felt artificially blocked at every plateau (real user feedback
       *  on 2026-05-09). Click still works as a manual fallback. */}
      {remainingCount > 0 && (
        <button
          ref={loadMoreRef}
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
