import { Filter, ArrowUpDown, Download, Upload, RefreshCw } from 'lucide-react';
import { mcps as mcpsApi } from '../../lib/api';
import { ListControls } from '../ListControls';
import type { McpPageState } from './useMcpPageState';

/** Filter/sort options panel + sync/import/export actions, rendered above
 *  the plugin list (`CollectionShell`'s `afterSidebarHeader` slot).
 *  Extracted verbatim from the pre-KT-830 `McpPage` render. */
export function PluginToolbarPanel({ state }: { state: McpPageState }) {
  const {
    t, totalConfigs, mcpSearchPanel,
    mcpKindFilter, setMcpKindFilter,
    mcpSort, setMcpSort, mcpSortReversed, setMcpSortReversed,
    servers, globalConfigs,
    setPortabilityMode, syncing, setSyncing, refetchMcps,
  } = state;

  return (
    <>
      {totalConfigs > 1 && mcpSearchPanel === 'filters' && (
        <div id="mcp-filter-options" className="collection-shell-search-options">
          <div className="list-controls">
            <label className="list-control">
              <Filter size={12} aria-hidden="true" />
              <span>{t('automation.filter.label')}</span>
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
          <button type="button" className="collection-shell-icon" disabled={syncing} onClick={async () => { setSyncing(true); try { await mcpsApi.refresh(); refetchMcps(); } catch (e) { console.warn('Failed to sync MCPs:', e); } finally { setSyncing(false); } }} aria-label={t('mcp.detect')} title={t('mcp.detect')}><RefreshCw size={14} className={syncing ? 'spin' : ''} /></button>
        </div>
      </div>
    </>
  );
}

/** Filter / sort toggle icons, rendered at the end of the sidebar header
 *  (`CollectionShell`'s `sidebarHeaderEnd` slot) — only shown once there's
 *  more than one plugin to filter or sort. */
export function PluginToolbarToggle({ state }: { state: McpPageState }) {
  const {
    t, totalConfigs, mcpSearchPanel, setMcpSearchPanel,
    mcpKindFilter, mcpSort, mcpSortReversed,
  } = state;

  if (totalConfigs <= 1) return null;

  return (
    <>
      <button
        type="button"
        className="collection-shell-search-action collection-shell-search-action-icon"
        data-active={mcpSearchPanel === 'filters' || mcpKindFilter !== 'all'}
        onClick={() => setMcpSearchPanel(panel => panel === 'filters' ? null : 'filters')}
        aria-label={t('mcp.filterKind')}
        aria-expanded={mcpSearchPanel === 'filters'}
        aria-controls={mcpSearchPanel === 'filters' ? 'mcp-filter-options' : undefined}
        title={t('mcp.filterKind')}
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
