import { useEffect, useId, useRef } from 'react';
import { MessageSquareText, PlugZap, Search, TerminalSquare, Workflow as WorkflowIcon, X, type LucideIcon } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import {
  activeAutomationFilterCount,
  AUTOMATION_KIND_FILTERS,
  AUTOMATION_NO_PROJECT,
  AUTOMATION_STATE_FILTERS,
  type AutomationFilters,
  type AutomationKind,
  type AutomationKindFilter,
  type AutomationStateFilter,
} from '../lib/automationFilters';
import { FilterFold } from './FilterFold';
import './AutomationFilterBar.css';

/** The tour anchors each type chip; they are written out so the tour test can
 *  find them in the source. */
const KIND_CHIP = {
  workflows: { icon: WorkflowIcon, label: 'wf.tabWorkflows', anchor: { 'data-tour-id': 'automation-kind-workflow' } },
  quickApis: { icon: PlugZap, label: 'wf.tabQuickApis', anchor: { 'data-tour-id': 'automation-kind-quick-api' } },
  quickPrompts: { icon: MessageSquareText, label: 'wf.tabQuickPrompts', anchor: { 'data-tour-id': 'automation-kind-quick-prompt' } },
  quickExecs: { icon: TerminalSquare, label: 'wf.tabQuickExecs', anchor: { 'data-tour-id': 'automation-kind-quick-exec' } },
} as const satisfies Record<AutomationKind, { icon: LucideIcon; label: string; anchor: Record<string, string> }>;

interface AutomationFilterBarProps {
  filters: AutomationFilters;
  /** What each type chip would show given the other filters and the search. */
  kindCounts: Record<AutomationKindFilter, number>;
  stateCounts: Record<AutomationStateFilter, number>;
  projects: Array<{ id: string; name: string }>;
  onQueryChange: (query: string) => void;
  onKindChange: (kind: AutomationKindFilter) => void;
  onStateChange: (state: AutomationStateFilter) => void;
  onProjectChange: (projectId: string) => void;
  /** Resets the type, state and project filters; the search keeps its own ✕. */
  onClear: () => void;
}

/**
 * The filters of the global Automation page — search, type, state, project —
 * in one bar above the list, where the sidebar used to mix chips and a filter
 * button into its own header. The sidebar keeps the project tree; a filter set
 * here narrows both the tree and the list. On a narrow screen the type, state
 * and project controls fold behind one "Filters (n)" button.
 */
export function AutomationFilterBar({
  filters, kindCounts, stateCounts, projects,
  onQueryChange, onKindChange, onStateChange, onProjectChange, onClear,
}: AutomationFilterBarProps) {
  const { t } = useT();
  const searchId = useId();
  const searchRef = useRef<HTMLInputElement>(null);
  const activeCount = activeAutomationFilterCount(filters);

  // `/` reaches the search from anywhere on the page, as in the other collections.
  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      const target = event.target;
      if (event.key !== '/' || (target instanceof HTMLElement && target.matches('input, textarea, select, [contenteditable="true"]'))) return;
      event.preventDefault();
      searchRef.current?.focus();
    };
    window.addEventListener('keydown', focusSearch);
    return () => window.removeEventListener('keydown', focusSearch);
  }, []);

  return (
    <div className="automation-filterbar" role="search" aria-label={t('automation.filtersBar')} data-testid="automation-filterbar">
      <div className="automation-filterbar-search">
        <Search size={13} className="automation-filterbar-search-icon" aria-hidden="true" />
        <input
          id={searchId}
          ref={searchRef}
          className="automation-filterbar-input"
          value={filters.query}
          onChange={event => onQueryChange(event.target.value)}
          aria-label={t('automation.search')}
          aria-keyshortcuts="/"
          placeholder={t('automation.search')}
        />
        {filters.query && (
          <button
            type="button"
            className="automation-filterbar-clear-search"
            onClick={() => onQueryChange('')}
            aria-label={t('automation.clearSearch')}
            title={t('automation.clearSearch')}
          >
            <X size={10} aria-hidden="true" />
          </button>
        )}
      </div>

      <FilterFold label={t('collection.filters')} activeCount={activeCount} className="automation-filterbar-fold">
        <div className="automation-filterbar-group" role="group" aria-label={t('automation.typeFilter')}>
          <button
            type="button"
            className="collection-shell-filter"
            data-active={filters.kind === 'all'}
            aria-pressed={filters.kind === 'all'}
            onClick={() => onKindChange('all')}
          >
            {t('automation.allTypes')} ({kindCounts.all})
          </button>
          {AUTOMATION_KIND_FILTERS.map(kind => {
            const { icon: Icon, label, anchor } = KIND_CHIP[kind];
            return (
              <button
                key={kind}
                type="button"
                className="collection-shell-filter"
                {...anchor}
                data-active={filters.kind === kind}
                aria-pressed={filters.kind === kind}
                onClick={() => onKindChange(kind)}
              >
                <Icon size={11} aria-hidden="true" />
                {t(label)} ({kindCounts[kind]})
              </button>
            );
          })}
        </div>

        <div className="automation-filterbar-group" role="group" aria-label={t('automation.stateFilter')}>
          {AUTOMATION_STATE_FILTERS.map(state => (
            <button
              key={state}
              type="button"
              className="collection-shell-filter"
              data-state-filter={state}
              data-active={filters.state === state}
              aria-pressed={filters.state === state}
              onClick={() => onStateChange(state)}
            >
              {t(`automation.state.${state}`)} ({stateCounts[state]})
            </button>
          ))}
        </div>

        <select
          className="automation-project-filter"
          value={filters.projectId}
          onChange={event => onProjectChange(event.target.value)}
          aria-label={t('automation.projectFilter')}
        >
          <option value="all">{t('automation.allProjects')}</option>
          <option value={AUTOMATION_NO_PROJECT}>{t('disc.noProject')}</option>
          {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
        </select>

        {activeCount > 0 && (
          <button type="button" className="collection-shell-clear automation-filterbar-clear" onClick={onClear}>
            {t('collection.clearFilters')}
          </button>
        )}
      </FilterFold>
    </div>
  );
}
