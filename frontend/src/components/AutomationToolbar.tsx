import type { Dispatch, SetStateAction } from 'react';
import { ArrowUpDown, Filter, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import {
  AUTOMATION_KIND_FILTERS,
  AUTOMATION_NO_PROJECT,
  AUTOMATION_STATE_FILTERS,
  activeAutomationFilterCount,
  type AutomationFilters,
  type AutomationKind,
  type AutomationKindFilter,
  type AutomationStateFilter,
} from '../lib/automationFilters';
import type { AutomationSort } from '../lib/automationSort';
import { ListControls } from './ListControls';
import './AutomationToolbar.css';

export type AutomationSearchPanel = 'filters' | 'sort' | null;

const KIND_LABEL = {
  workflows: 'wf.tabWorkflows',
  quickApis: 'wf.tabQuickApis',
  quickPrompts: 'wf.tabQuickPrompts',
  quickExecs: 'wf.tabQuickExecs',
} as const satisfies Record<AutomationKind, string>;

export interface AutomationToolbarState {
  filters: AutomationFilters;
  /** What each option would show given the other filters and the search. */
  kindCounts: Record<AutomationKindFilter, number>;
  stateCounts: Record<AutomationStateFilter, number>;
  projects: Array<{ id: string; name: string }>;
  panel: AutomationSearchPanel;
  setPanel: Dispatch<SetStateAction<AutomationSearchPanel>>;
  sort: AutomationSort;
  setSort: (sort: AutomationSort) => void;
  sortReversed: boolean;
  setSortReversed: Dispatch<SetStateAction<boolean>>;
  onKindChange: (kind: AutomationKindFilter) => void;
  onStateChange: (state: AutomationStateFilter) => void;
  onProjectChange: (projectId: string) => void;
  /** Resets the type, state and project filters; the search keeps its own ✕. */
  onClear: () => void;
}

/** Filter / sort options panels, rendered under the sidebar search
 *  (`CollectionShell`'s `afterSidebarHeader` slot) — the same layout as the
 *  Plugins page: one full-width `<select>` per filter, stacked. */
export function AutomationToolbarPanel({ toolbar }: { toolbar: AutomationToolbarState }) {
  const { t } = useT();
  const {
    filters, kindCounts, stateCounts, projects, panel,
    sort, setSort, sortReversed, setSortReversed,
    onKindChange, onStateChange, onProjectChange, onClear,
  } = toolbar;
  const activeCount = activeAutomationFilterCount(filters);

  return (
    <>
      {panel === 'filters' && (
        <div id="automation-filter-options" className="collection-shell-search-options">
          <div className="automation-filter-stack">
            <label className="automation-filter-field">
              <span>{t('automation.filter.type')}</span>
              <select
                data-tour-id="automation-filter-type"
                value={filters.kind}
                onChange={event => onKindChange(event.target.value as AutomationKindFilter)}
                aria-label={t('automation.typeFilter')}
              >
                <option value="all">{t('automation.allTypes')} ({kindCounts.all})</option>
                {AUTOMATION_KIND_FILTERS.map(kind => (
                  <option key={kind} value={kind}>{t(KIND_LABEL[kind])} ({kindCounts[kind]})</option>
                ))}
              </select>
            </label>
            <label className="automation-filter-field">
              <span>{t('automation.filter.state')}</span>
              <select
                value={filters.state}
                onChange={event => onStateChange(event.target.value as AutomationStateFilter)}
                aria-label={t('automation.stateFilter')}
              >
                {AUTOMATION_STATE_FILTERS.map(state => (
                  <option key={state} value={state}>{t(`automation.state.${state}`)} ({stateCounts[state]})</option>
                ))}
              </select>
            </label>
            <label className="automation-filter-field">
              <span>{t('automation.filter.project')}</span>
              <select
                value={filters.projectId}
                onChange={event => onProjectChange(event.target.value)}
                aria-label={t('automation.projectFilter')}
              >
                <option value="all">{t('automation.allProjects')}</option>
                <option value={AUTOMATION_NO_PROJECT}>{t('disc.noProject')}</option>
                {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
              </select>
            </label>
            {activeCount > 0 && (
              <button type="button" className="automation-filter-clear" onClick={onClear}>
                <X size={11} aria-hidden="true" />{t('collection.clearFilters')}
              </button>
            )}
          </div>
        </div>
      )}
      {panel === 'sort' && (
        <div id="automation-sort-options" className="collection-shell-search-options">
          <ListControls
            sortLabel={t('automation.sort.label')}
            sortAriaLabel={t('automation.sortLabel')}
            sortValue={sort}
            sortOptions={[
              { value: 'name', label: t('automation.sort.name') },
              { value: 'updated', label: t('automation.sort.updated') },
              { value: 'kind', label: t('automation.sort.kind') },
            ]}
            onSortChange={setSort}
            reversed={sortReversed}
            onToggleDirection={() => setSortReversed(value => !value)}
            directionLabel={t(sortReversed
              ? 'automation.sort.restoreDirection'
              : 'automation.sort.reverseDirection')}
          />
        </div>
      )}
    </>
  );
}

/** Filter / sort toggle icons, rendered at the end of the sidebar search row
 *  (`CollectionShell`'s `sidebarHeaderEnd` slot). The icon stays lit while a
 *  filter is set or the order is not the default one. */
export function AutomationToolbarToggle({ toolbar }: { toolbar: AutomationToolbarState }) {
  const { t } = useT();
  const { filters, panel, setPanel, sort, sortReversed } = toolbar;

  return (
    <>
      <button
        type="button"
        className="collection-shell-search-action collection-shell-search-action-icon"
        data-tour-id="automation-filters"
        data-active={panel === 'filters' || activeAutomationFilterCount(filters) > 0}
        onClick={() => setPanel(current => current === 'filters' ? null : 'filters')}
        aria-label={t('automation.filters')}
        aria-expanded={panel === 'filters'}
        aria-controls={panel === 'filters' ? 'automation-filter-options' : undefined}
        title={t('automation.filters')}
      >
        <Filter size={14} aria-hidden="true" />
      </button>
      <button
        type="button"
        className="collection-shell-search-action collection-shell-search-action-icon"
        data-active={panel === 'sort' || sort !== 'name' || sortReversed}
        onClick={() => setPanel(current => current === 'sort' ? null : 'sort')}
        aria-label={t('automation.sortLabel')}
        aria-expanded={panel === 'sort'}
        aria-controls={panel === 'sort' ? 'automation-sort-options' : undefined}
        title={t('automation.sortLabel')}
      >
        <ArrowUpDown size={14} aria-hidden="true" />
      </button>
    </>
  );
}
