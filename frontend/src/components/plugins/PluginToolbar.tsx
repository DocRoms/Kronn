import { Filter, ArrowUpDown, Download, Upload, RefreshCw, X } from 'lucide-react';
import { ListControls } from '../ListControls';
import type { McpPageState } from './useMcpPageState';

/** Filters stay reachable while one is set, even when the list has shrunk to
 *  a single plugin — otherwise a project filter could not be cleared. */
const filtersAvailable = (state: McpPageState) => state.totalConfigs > 1 || state.activeFilterCount > 0;

/** Filter/sort options panel + sync/import/export actions, rendered above
 *  the plugin list (`CollectionShell`'s `afterSidebarHeader` slot).
 *  Extracted verbatim from the pre-KT-830 `McpPage` render, then given the
 *  Project / Health / Local sync filters (KT-907) that replace the project
 *  tree the sidebar used to carry. */
export function PluginToolbarPanel({ state }: { state: McpPageState }) {
  const {
    t, totalConfigs, mcpSearchPanel, projects,
    mcpKindFilter, setMcpKindFilter,
    selectedProjectId, setSelectedProjectId,
    mcpHealthFilter, setMcpHealthFilter, mcpSyncFilter, setMcpSyncFilter,
    activeFilterCount, clearPluginFilters,
    mcpSort, setMcpSort, mcpSortReversed, setMcpSortReversed,
    servers, globalConfigs,
    setPortabilityMode, syncing, handlePreviewRescan, setSelectedConfigId,
  } = state;

  return (
    <>
      {filtersAvailable(state) && mcpSearchPanel === 'filters' && (
        <div id="mcp-filter-options" className="collection-shell-search-options">
          <div className="mcp-filter-stack">
            <label className="mcp-filter-field">
              <span>{t('mcp.kindFilter.label')}</span>
              <select
                value={mcpKindFilter}
                onChange={event => setMcpKindFilter(event.target.value as typeof mcpKindFilter)}
                aria-label={t('mcp.filterKind')}
              >
                <option value="all">{t('mcp.kindFilter.all')}</option>
                <option value="mcp">{t('mcp.kindFilter.mcp')}</option>
                <option value="api">{t('mcp.kindFilter.api')}</option>
                <option value="cli">{t('mcp.kindFilter.cli')}</option>
              </select>
            </label>
            <label className="mcp-filter-field">
              <span>{t('mcp.filter.project')}</span>
              <select
                value={selectedProjectId}
                onChange={event => setSelectedProjectId(event.target.value)}
                aria-label={t('mcp.filter.project')}
              >
                <option value="__all__">{t('mcp.filter.projectAll')}</option>
                <option value="__none__">{t('disc.noProject')}</option>
                {projects.map(project => <option key={project.id} value={project.id}>{project.name}</option>)}
              </select>
            </label>
            <label className="mcp-filter-field">
              <span>{t('mcp.filter.health')}</span>
              <select
                value={mcpHealthFilter}
                onChange={event => setMcpHealthFilter(event.target.value as typeof mcpHealthFilter)}
                aria-label={t('mcp.filter.health')}
              >
                <option value="all">{t('mcp.filter.healthAll')}</option>
                <option value="error">{t('mcp.health.error')}</option>
                <option value="warning">{t('mcp.health.warning')}</option>
                <option value="ok">{t('mcp.health.ok')}</option>
              </select>
            </label>
            <label className="mcp-filter-field">
              <span>{t('mcp.filter.sync')}</span>
              <select
                value={mcpSyncFilter}
                onChange={event => setMcpSyncFilter(event.target.value as typeof mcpSyncFilter)}
                aria-label={t('mcp.filter.sync')}
              >
                <option value="all">{t('mcp.filter.syncAll')}</option>
                <option value="local">{t('mcp.filter.syncLocal')}</option>
                <option value="none">{t('mcp.filter.syncNone')}</option>
              </select>
            </label>
            {activeFilterCount > 0 && (
              <button type="button" className="mcp-filter-clear" onClick={clearPluginFilters}>
                <X size={11} aria-hidden="true" />{t('collection.clearFilters')}
              </button>
            )}
          </div>
        </div>
      )}
      {totalConfigs > 1 && mcpSearchPanel === 'sort' && (
        <div id="mcp-sort-options" className="collection-shell-search-options">
          <ListControls
            sortLabel={t('automation.sort.label')}
            sortAriaLabel={t('mcp.sortLabel')}
            sortValue={mcpSort}
            sortOptions={[
              { value: 'name', label: t('automation.sort.name') },
              { value: 'kind', label: t('mcp.sortKind') },
              { value: 'scope', label: t('mcp.sortScope') },
            ]}
            onSortChange={setMcpSort}
            reversed={mcpSortReversed}
            onToggleDirection={() => setMcpSortReversed(value => !value)}
            directionLabel={t(mcpSortReversed
              ? 'automation.sort.restoreDirection'
              : 'automation.sort.reverseDirection')}
          />
        </div>
      )}
      <div className="mcp-collection-toolbar">
        <span className="mcp-meta">{servers.length} {servers.length > 1 ? t('mcp.serverPlural') : t('mcp.server')} · {globalConfigs.length} {globalConfigs.length > 1 ? t('mcp.globalPlural') : t('mcp.global')}</span>
        <div className="mcp-collection-toolbar-actions">
          {totalConfigs > 0 && <button type="button" className="collection-shell-icon" onClick={() => setPortabilityMode('export')} aria-label={t('mcp.portability.export')} title={t('mcp.portability.exportTitle')}><Download size={14} /></button>}
          <button type="button" className="collection-shell-icon" onClick={() => setPortabilityMode('import')} aria-label={t('mcp.portability.import')} title={t('mcp.portability.importTitle')}><Upload size={14} /></button>
          <button type="button" className="collection-shell-icon" disabled={syncing} onClick={() => { setSelectedConfigId(null); void handlePreviewRescan(); }} aria-label={t('mcp.detect')} title={t('mcp.detect')}><RefreshCw size={14} className={syncing ? 'spin' : ''} /></button>
        </div>
      </div>
    </>
  );
}

/** Filter / sort toggle icons, rendered at the end of the sidebar header
 *  (`CollectionShell`'s `sidebarHeaderEnd` slot) — only shown once there's
 *  more than one plugin to filter or sort, or while a filter is set. */
export function PluginToolbarToggle({ state }: { state: McpPageState }) {
  const {
    t, mcpSearchPanel, setMcpSearchPanel,
    activeFilterCount, mcpSort, mcpSortReversed,
  } = state;

  if (!filtersAvailable(state)) return null;

  return (
    <>
      <button
        type="button"
        className="collection-shell-search-action collection-shell-search-action-icon"
        data-active={mcpSearchPanel === 'filters' || activeFilterCount > 0}
        onClick={() => setMcpSearchPanel(panel => panel === 'filters' ? null : 'filters')}
        aria-label={t('mcp.filters')}
        aria-expanded={mcpSearchPanel === 'filters'}
        aria-controls={mcpSearchPanel === 'filters' ? 'mcp-filter-options' : undefined}
        title={t('mcp.filters')}
      >
        <Filter size={14} aria-hidden="true" />
      </button>
      <button
        type="button"
        className="collection-shell-search-action collection-shell-search-action-icon"
        data-active={mcpSearchPanel === 'sort' || mcpSort !== 'name' || mcpSortReversed}
        onClick={() => setMcpSearchPanel(panel => panel === 'sort' ? null : 'sort')}
        aria-label={t('mcp.sortLabel')}
        aria-expanded={mcpSearchPanel === 'sort'}
        aria-controls={mcpSearchPanel === 'sort' ? 'mcp-sort-options' : undefined}
        title={t('mcp.sortLabel')}
      >
        <ArrowUpDown size={14} aria-hidden="true" />
      </button>
    </>
  );
}
