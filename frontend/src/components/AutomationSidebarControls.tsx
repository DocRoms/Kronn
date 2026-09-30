import { useEffect, useId, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { Check, ChevronDown, Plus, Star, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import {
  AUTOMATION_GROUP_BYS,
  AUTOMATION_KIND_FILTERS,
  AUTOMATION_KIND_LABEL_KEYS,
  AUTOMATION_NO_PROJECT,
  activeAutomationFilterCount,
  type AutomationFilters,
  type AutomationGroupBy,
  type AutomationKindFilter,
} from '../lib/automationFilters';
import type { AutomationSort } from '../lib/automationSort';
import './AutomationSidebarControls.css';

const GROUP_BY_LABEL_KEYS = {
  kind: 'automation.groupBy.kind',
  project: 'automation.groupBy.project',
  none: 'automation.groupBy.none',
} as const satisfies Record<AutomationGroupBy, string>;

export interface AutomationControlsState {
  groupBy: AutomationGroupBy;
  onGroupByChange: (groupBy: AutomationGroupBy) => void;
  filters: AutomationFilters;
  /** What each type would show given the other filters and the search. */
  kindCounts: Record<AutomationKindFilter, number>;
  /** Same, per project id and `AUTOMATION_NO_PROJECT`. */
  projectCounts: Record<string, number>;
  projects: Array<{ id: string; name: string }>;
  onKindChange: (kind: AutomationKindFilter) => void;
  onPinnedChange: (pinned: boolean) => void;
  onActiveChange: (active: boolean) => void;
  onRecentChange: (recent: boolean) => void;
  onProjectChange: (projectId: string) => void;
  /** Resets every chip; the search keeps its own ✕. */
  onClear: () => void;
}

type OpenMenu = 'kind' | 'project' | null;

/** "Group by" segmented control and the filter chips, rendered under the
 *  sidebar search (`CollectionShell`'s `afterSidebarHeader` slot): a type chip
 *  that reads "All" until a type is picked, the Pinned / Active / Recent
 *  toggles, and a removable chip for the chosen project. */
export function AutomationSidebarControls({ controls }: { controls: AutomationControlsState }) {
  const { t } = useT();
  const {
    groupBy, onGroupByChange, filters, kindCounts, projectCounts, projects,
    onKindChange, onPinnedChange, onActiveChange, onRecentChange, onProjectChange, onClear,
  } = controls;
  const [menu, setMenu] = useState<OpenMenu>(null);
  const groupByLabelId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const kindChipRef = useRef<HTMLButtonElement>(null);
  const projectChipRef = useRef<HTMLButtonElement>(null);

  const kindLabel = (kind: AutomationKindFilter) => (
    kind === 'all' ? t('automation.allTypes') : t(AUTOMATION_KIND_LABEL_KEYS[kind])
  );
  const projectName = (projectId: string) => (
    projectId === AUTOMATION_NO_PROJECT
      ? t('disc.noProject')
      : projects.find(project => project.id === projectId)?.name ?? projectId
  );
  const hasProject = filters.projectId !== 'all';

  useEffect(() => {
    if (!menu) return;
    const close = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setMenu(null);
    };
    // Capture phase, and stopped: on a narrow screen the sidebar is a drawer
    // that Escape also dismisses, and this Escape only means "close the menu".
    const closeFromKeyboard = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      setMenu(null);
      (menu === 'kind' ? kindChipRef : projectChipRef).current?.focus();
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', closeFromKeyboard, true);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', closeFromKeyboard, true);
    };
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const options = menuRef.current?.querySelectorAll<HTMLElement>('[role="option"]');
    const selected = menuRef.current?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]');
    (selected ?? options?.[0])?.focus();
  }, [menu]);

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    // The sidebar moves between list rows with the same keys: not from here.
    event.stopPropagation();
    const options = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="option"]'));
    if (options.length === 0) return;
    const current = options.indexOf(document.activeElement as HTMLElement);
    const next = event.key === 'Home' ? 0
      : event.key === 'End' ? options.length - 1
        : (current + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length;
    event.preventDefault();
    options[next]?.focus();
  };

  const toggleMenu = (which: Exclude<OpenMenu, null>) => setMenu(current => (current === which ? null : which));
  // Focus goes back to the chip once it has re-rendered: choosing a project
  // swaps "+ Project" for the removable chip, which is a different button.
  const choose = (apply: () => void, chip: RefObject<HTMLButtonElement | null>) => {
    apply();
    setMenu(null);
    requestAnimationFrame(() => chip.current?.focus());
  };

  const projectOptions = [AUTOMATION_NO_PROJECT, ...projects.map(project => project.id)]
    .filter(id => (projectCounts[id] ?? 0) > 0 || id === filters.projectId);

  return (
    <div className="automation-controls" ref={rootRef}>
      <div className="automation-groupby">
        <span className="automation-groupby-label" id={groupByLabelId}>{t('automation.groupBy.label')}</span>
        <div className="automation-segmented" role="group" aria-labelledby={groupByLabelId}>
          {AUTOMATION_GROUP_BYS.map(by => (
            <button
              key={by}
              type="button"
              className="automation-segmented-option"
              data-active={groupBy === by}
              aria-pressed={groupBy === by}
              onClick={() => onGroupByChange(by)}
            >{t(GROUP_BY_LABEL_KEYS[by])}</button>
          ))}
        </div>
      </div>

      <div className="automation-chips" data-tour-id="automation-filters" role="group" aria-label={t('automation.filters')}>
        <button
          ref={kindChipRef}
          type="button"
          className="automation-chip"
          data-tour-id="automation-filter-type"
          data-value={filters.kind}
          data-active={filters.kind !== 'all'}
          aria-haspopup="listbox"
          aria-expanded={menu === 'kind'}
          aria-label={t('automation.chip.kind', kindLabel(filters.kind))}
          onClick={() => toggleMenu('kind')}
        >
          {kindLabel(filters.kind)}
          <ChevronDown size={11} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="automation-chip"
          data-active={filters.pinned}
          aria-pressed={filters.pinned}
          onClick={() => onPinnedChange(!filters.pinned)}
        >
          <Star size={11} fill={filters.pinned ? 'currentColor' : 'none'} aria-hidden="true" />
          {t('automation.chip.pinned')}
        </button>
        <button
          type="button"
          className="automation-chip"
          data-active={filters.active}
          aria-pressed={filters.active}
          onClick={() => onActiveChange(!filters.active)}
        >{t('automation.chip.active')}</button>
        <button
          type="button"
          className="automation-chip"
          data-active={filters.recent}
          aria-pressed={filters.recent}
          onClick={() => onRecentChange(!filters.recent)}
        >{t('disc.recent')}</button>
        {hasProject ? (
          <span className="automation-chip automation-chip-project" data-active="true">
            <button
              ref={projectChipRef}
              type="button"
              className="automation-chip-project-name"
              aria-haspopup="listbox"
              aria-expanded={menu === 'project'}
              title={t('automation.projectFilter')}
              onClick={() => toggleMenu('project')}
            >{projectName(filters.projectId)}</button>
            <button
              type="button"
              className="automation-chip-project-remove"
              aria-label={t('automation.chip.removeProject', projectName(filters.projectId))}
              onClick={() => { setMenu(null); onProjectChange('all'); }}
            ><X size={11} aria-hidden="true" /></button>
          </span>
        ) : (
          <button
            ref={projectChipRef}
            type="button"
            className="automation-chip automation-chip-add"
            aria-haspopup="listbox"
            aria-expanded={menu === 'project'}
            aria-label={t('automation.projectFilter')}
            onClick={() => toggleMenu('project')}
          >
            <Plus size={11} aria-hidden="true" />
            {t('automation.chip.project')}
          </button>
        )}
        {activeAutomationFilterCount(filters) > 0 && (
          <button type="button" className="automation-chip-clear" onClick={() => { setMenu(null); onClear(); }}>
            <X size={11} aria-hidden="true" />{t('collection.clearFilters')}
          </button>
        )}
      </div>

      {menu === 'kind' && (
        <div className="automation-chip-menu" role="listbox" ref={menuRef} aria-label={t('automation.typeFilter')} onKeyDown={onMenuKeyDown}>
          {(['all', ...AUTOMATION_KIND_FILTERS] as AutomationKindFilter[]).map(kind => (
            <button
              key={kind}
              type="button"
              role="option"
              className="automation-chip-option"
              data-kind-option={kind}
              aria-selected={filters.kind === kind}
              aria-label={`${kindLabel(kind)} (${kindCounts[kind]})`}
              onClick={() => choose(() => onKindChange(kind), kindChipRef)}
            >
              <span>{kindLabel(kind)}</span>
              <span className="automation-chip-option-count" aria-hidden="true">{kindCounts[kind]}</span>
            </button>
          ))}
        </div>
      )}
      {menu === 'project' && (
        <div className="automation-chip-menu" role="listbox" ref={menuRef} aria-label={t('automation.projectFilter')} onKeyDown={onMenuKeyDown}>
          {projectOptions.map(id => (
            <button
              key={id}
              type="button"
              role="option"
              className="automation-chip-option"
              data-project-option={id}
              aria-selected={filters.projectId === id}
              aria-label={`${projectName(id)} (${projectCounts[id] ?? 0})`}
              onClick={() => choose(() => onProjectChange(id), projectChipRef)}
            >
              <span>{projectName(id)}</span>
              <span className="automation-chip-option-count" aria-hidden="true">{projectCounts[id] ?? 0}</span>
            </button>
          ))}
          {projectOptions.length === 0 && (
            <p className="automation-chip-menu-empty">{t('automation.chip.noProjects')}</p>
          )}
        </div>
      )}
    </div>
  );
}

const SORT_OPTIONS = [
  ['name', 'automation.sort.name'],
  ['updated', 'automation.sort.updated'],
  ['opened', 'automation.sort.opened'],
] as const satisfies ReadonlyArray<readonly [AutomationSort, string]>;

/** The sort choices, rendered in the sidebar's ⋯ menu (`CollectionShell`'s
 *  `moreActionsMenuExtra` slot). The menu closes itself on any click. */
export function AutomationSortMenuItems({ sort, onSortChange, reversed, onReversedChange }: {
  sort: AutomationSort;
  onSortChange: (sort: AutomationSort) => void;
  reversed: boolean;
  onReversedChange: (reversed: boolean) => void;
}) {
  const { t } = useT();
  return (
    <div className="automation-sort-menu" role="group" aria-label={t('automation.sortLabel')}>
      <span className="automation-sort-menu-title" aria-hidden="true">{t('automation.sort.label')}</span>
      {SORT_OPTIONS.map(([value, labelKey]) => (
        <button
          key={value}
          type="button"
          role="menuitemradio"
          aria-checked={sort === value}
          onClick={() => onSortChange(value)}
        >
          <span className="automation-sort-menu-check" aria-hidden="true">{sort === value && <Check size={12} />}</span>
          {t(labelKey)}
        </button>
      ))}
      <button
        type="button"
        role="menuitemcheckbox"
        aria-checked={reversed}
        onClick={() => onReversedChange(!reversed)}
      >
        <span className="automation-sort-menu-check" aria-hidden="true">{reversed && <Check size={12} />}</span>
        {t(reversed ? 'automation.sort.restoreDirection' : 'automation.sort.reverseDirection')}
      </button>
    </div>
  );
}
