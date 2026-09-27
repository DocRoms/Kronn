import type { ReactNode } from 'react';
import { Puzzle, Plus, Trash2, ChevronRight, Folder, Globe, Plug, AlertTriangle, Key, CheckSquare } from 'lucide-react';
import type { McpConfigDisplay } from '../../types/generated';
import { CollectionShell } from '../CollectionShell';
import { CollectionFavoritesHeader } from '../CollectionFavoritesHeader';
import { CollectionRowActions } from '../CollectionRowActions';
import { CollectionSidebarFooter } from '../CollectionSidebarFooter';
import { ContextHelp } from '../ContextHelp';
import { MatrixText } from '../MatrixText';
import { HostSyncChip } from '../HostSyncChip';
import { PluginKindBadge } from './PluginKindBadge';
import { PluginDetailPanel } from './PluginDetailPanel';
import { PluginToolbarPanel, PluginToolbarToggle } from './PluginToolbar';
import { hasAgentScope } from './mcpPageHelpers';
import type { McpPageState } from './useMcpPageState';

/** Installed-plugins grid: the `CollectionShell` wiring (sidebar list +
 *  inline detail) plus the built-in "kronn-internal" fallback tile.
 *  Extracted verbatim from the pre-KT-830 `McpPage` render — this is
 *  the "liste" piece of the KT-830 split. */
export function PluginCollectionView({ state }: { state: McpPageState }) {
  const {
    t, isMobile, projects,
    totalConfigs, visibleConfigs, favoriteConfigIds, toggleConfigFavorite,
    mcpSearch, setMcpSearch, selectedConfigId, setSelectedConfigId,
    selectedConfigIds, setSelectedConfigIds, handleDeleteSelectedMcpConfigs,
    sidebarOpen, setSidebarOpen,
    isBuiltinConfig, driftBySlug,
    collapsedMcpGroups, setCollapsedMcpGroups,
    handleDeleteMcpConfig, showBuiltinFallback,
    setShowAddMcp, setAddMcpSelected, setAddMcpSearch, addMcpTriggerRef,
  } = state;

  return (
    <div className="mcp-plugin-shell">
      <CollectionShell<McpConfigDisplay>
        ariaLabel={t('mcp.title')}
        title={<><Puzzle size={17} /> <MatrixText text={t('mcp.title')} /></>}
        titleCount={totalConfigs}
        headerActions={<>
          <ContextHelp title={t('contextHelp.plugins.title')}>
            <p>{t('contextHelp.plugins.intro')}</p>
            <ul><li>{t('contextHelp.plugins.mcp')}</li><li>{t('contextHelp.plugins.api')}</li><li>{t('contextHelp.plugins.cli')}</li></ul>
            <p className="kr-context-help-agent-note">{t('contextHelp.plugins.agents')}</p>
          </ContextHelp>
          <button ref={addMcpTriggerRef} type="button" className="collection-shell-icon collection-shell-primary-action" data-tour-id="add-plugin-btn" onClick={() => { setShowAddMcp(true); setAddMcpSelected(null); setAddMcpSearch(''); }} aria-label={t('mcp.add')} title={t('mcp.addTitle')}><Plus size={16} /></button>
        </>}
        items={visibleConfigs}
        getId={config => config.id}
        getLabel={config => `${config.label} ${config.server_name} ${config.project_names.join(' ')}`}
        isFavorite={config => favoriteConfigIds.has(config.id)}
        onToggleFavorite={config => toggleConfigFavorite(config.id)}
        persistence={{
          query: mcpSearch,
          onQueryChange: setMcpSearch,
          favoritesOnly: false,
          onFavoritesOnlyChange: () => {},
        }}
        selectedId={selectedConfigId}
        onSelect={setSelectedConfigId}
        selectedIds={selectedConfigIds}
        onSelectedIdsChange={setSelectedConfigIds}
        actions={[
          {
            id: 'delete',
            label: t('collection.deleteSelected'),
            icon: <Trash2 size={15} />,
            danger: true,
            disabled: selected => selected.length === 0,
            onSelect: handleDeleteSelectedMcpConfigs,
          },
        ]}
        isMobile={isMobile}
        sidebarOpen={sidebarOpen}
        onSidebarOpenChange={setSidebarOpen}
        globalSearchShortcut
        showSearchClear
        showControls={false}
        labels={{
          search: t('mcp.search'),
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
          afterSidebarHeader: <PluginToolbarPanel state={state} />,
          sidebarHeaderEnd: <PluginToolbarToggle state={state} />,
          renderList: ({ visibleItems, getRowProps, canMultiSelect, isMultiSelected, toggleMultiSelection }) => {
            const isGroupCollapsed = (group: string) => (
              !canMultiSelect && !mcpSearch.trim() && collapsedMcpGroups.has(group)
            );
            const toggleGroup = (group: string) => {
              setCollapsedMcpGroups(current => {
                const next = new Set(current);
                if (next.has(group)) next.delete(group);
                else next.add(group);
                return next;
              });
            };
            const row = (config: McpConfigDisplay, keyPrefix: string) => {
              const rowProps = getRowProps(config);
              const kind = config.effective_kind;
              const isBuiltin = isBuiltinConfig(config);
              const selected = isMultiSelected(config);
              const scopeLabels = [
                config.is_global ? t('mcp.globalAll') : null,
                config.include_general ? t('disc.general') : null,
                config.project_ids.length > 0
                  ? `${config.project_ids.length} ${config.project_ids.length > 1 ? t('mcp.projectPlural') : t('mcp.project')}`
                  : null,
              ].filter((label): label is string => Boolean(label));
              const drifting = driftBySlug[config.server_id] ?? [];
              const worstDrift = drifting[0];
              return <div className="disc-swipe-wrap" key={`${keyPrefix}-${config.id}`}>
                <div
                  className="disc-item mcp-sidebar-plugin-row"
                  data-active={config.id === selectedConfigId}
                  data-selected={selected}
                  data-kind={kind}
                  data-config-id={config.id}
                  data-testid={isBuiltin ? 'mcp-kronn-internal-card' : undefined}
                >
                  <button
                    type="button"
                    {...rowProps}
                    className={`${rowProps.className} disc-item-open`}
                    onClick={canMultiSelect
                      ? () => toggleMultiSelection(config.id)
                      : rowProps.onClick}
                    aria-label={canMultiSelect
                      ? `${config.label} · ${t('collection.selectItem')}`
                      : `${config.label} — ${t('mcp.openDetails')}`}
                    role={canMultiSelect ? 'checkbox' : undefined}
                    aria-checked={canMultiSelect ? selected : undefined}
                  >
                    {canMultiSelect && <span className="disc-item-selection-box" data-selected={selected} aria-hidden="true">{selected && <CheckSquare size={12} />}</span>}
                    <span className="mcp-sidebar-plugin-icon" data-kind={kind} aria-hidden="true"><Puzzle size={14} /></span>
                    <span className="disc-item-content">
                      <span className="disc-item-title">
                        <span className="disc-item-title-text">{config.label}</span>
                        {isBuiltin && <span className="mcp-origin-badge mcp-origin-official">{t('mcp.builtin.tileBadge')}</span>}
                      </span>
                      <span className="disc-item-meta">
                        <span className="disc-item-meta-summary">
                          {config.server_name !== config.label ? `${config.server_name} · ` : ''}
                          {scopeLabels.length > 0 ? scopeLabels.join(' · ') : t('mcp.scopeOrphanShort')}
                        </span>
                        <PluginKindBadge kind={kind} />
                        {kind !== 'api' && <HostSyncChip mode={config.host_sync} />}
                        {config.env_keys.length > 0 && <span className="mcp-installed-keys"><Key size={9} /> {config.env_keys.length}</span>}
                        {config.secrets_broken && <span className="mcp-scope-badge mcp-scope-broken" title={t('mcp.secretsBroken')}>⚠ {t('mcp.secretsBrokenShort')}</span>}
                        {config.registry_drift && <span className="mcp-scope-badge mcp-scope-drift" title={t('mcp.registryDrift')}>⚠ {t(config.registry_drift.orphaned ? 'mcp.registryOrphanShort' : 'mcp.registryDriftShort')}</span>}
                        {worstDrift && <span
                          className="mcp-scope-badge mcp-scope-drift"
                          data-testid={`mcp-endpoint-drift-${config.server_id}`}
                          title={t(
                            worstDrift.successes > 0 ? 'mcp.drift.sometimes' : 'mcp.drift.never',
                            worstDrift.failures,
                            worstDrift.endpoint_path,
                            `HTTP ${worstDrift.http_status}`,
                          )}
                        >⚠ {t('mcp.drift.short')}</span>}
                      </span>
                    </span>
                  </button>
                  {!canMultiSelect && <CollectionRowActions
                    itemName={config.label}
                    favorite={{
                      active: favoriteConfigIds.has(config.id),
                      onToggle: () => toggleConfigFavorite(config.id),
                      activeLabel: t('disc.unpin'),
                      inactiveLabel: t('disc.pin'),
                    }}
                    menuLabel={t('collection.moreActions')}
                    copyId={config.id}
                    copyLabel={t('disc.copyId')}
                    actions={[{
                      id: 'delete',
                      label: t('mcp.deleteConfig'),
                      icon: <Trash2 size={12} />,
                      danger: true,
                      onSelect: () => handleDeleteMcpConfig(config.id),
                    }]}
                  />}
                </div>
              </div>;
            };

            const globalConfigs = visibleItems.filter(config => config.is_global);
            const generalConfigs = visibleItems.filter(config => config.include_general);
            const unassignedConfigs = visibleItems.filter(config => !hasAgentScope(
              config.is_global,
              config.include_general,
              config.project_ids,
            ));
            const projectGroups = new Map<string, { name: string; configs: McpConfigDisplay[] }>();
            const projectById = new Map(projects.map(project => [project.id, project]));
            for (const config of visibleItems) {
              config.project_ids.forEach((projectId, index) => {
                const knownProject = projectById.get(projectId);
                const group = projectGroups.get(projectId) ?? {
                  name: knownProject?.name ?? config.project_names[index] ?? projectId,
                  configs: [],
                };
                group.configs.push(config);
                projectGroups.set(projectId, group);
              });
            }
            const sortedProjectGroups = [...projectGroups.entries()].sort(([, left], [, right]) => (
              left.name.localeCompare(right.name, undefined, { sensitivity: 'base', numeric: true })
            ));
            const favorites = canMultiSelect
              ? []
              : visibleItems.filter(config => favoriteConfigIds.has(config.id));
            const renderGroup = (
              key: string,
              label: string,
              icon: ReactNode,
              groupConfigs: McpConfigDisplay[],
            ) => {
              if (groupConfigs.length === 0) return null;
              const collapsed = isGroupCollapsed(key);
              return <div key={key} data-mcp-group={key}>
                <button
                  type="button"
                  className="disc-group-btn"
                  onClick={() => toggleGroup(key)}
                  aria-expanded={!collapsed}
                >
                  <ChevronRight size={10} className="disc-chevron" data-expanded={!collapsed} />
                  {icon}<span>{label}</span><span className="disc-group-count">{groupConfigs.length}</span>
                </button>
                {!collapsed && groupConfigs.map(config => row(config, key))}
              </div>;
            };
            const projectsCollapsed = isGroupCollapsed('projects');
            const favoritesCollapsed = isGroupCollapsed('favorites');
            return <div className="disc-sidebar-list mcp-sidebar-items">
              {favorites.length > 0 && <div className="disc-sidebar-section disc-sidebar-favorites" data-expanded={!favoritesCollapsed}>
                <CollectionFavoritesHeader
                  label={t('disc.favorites')}
                  count={favorites.length}
                  expanded={!favoritesCollapsed}
                  onToggle={() => toggleGroup('favorites')}
                />
                {!favoritesCollapsed && favorites.map(config => row(config, 'favorite'))}
              </div>}

              {visibleItems.length > 0 && <div className="disc-sidebar-section disc-sidebar-projects" data-expanded={!projectsCollapsed}>
                <button type="button" className="disc-group-btn" data-no-border="true" onClick={() => toggleGroup('projects')} aria-expanded={!projectsCollapsed}>
                  <ChevronRight size={10} className="disc-chevron" data-expanded={!projectsCollapsed} />
                  <Folder size={10} /><span>{t('projects.title')}</span><span className="disc-group-count">{visibleItems.length}</span>
                </button>
                {!projectsCollapsed && <div className="disc-project-tree">
                  {renderGroup('global', t('mcp.globalAll'), <Globe size={10} />, globalConfigs)}
                  {renderGroup('general', t('disc.general'), <Plug size={10} />, generalConfigs)}
                  {sortedProjectGroups.map(([projectId, group]) => renderGroup(
                    `project:${projectId}`,
                    group.name,
                    <Folder size={10} />,
                    group.configs,
                  ))}
                  {renderGroup('unassigned', t('mcp.scopeOrphanShort'), <AlertTriangle size={10} />, unassignedConfigs)}
                </div>}
              </div>}

              {visibleItems.length === 0 && <div className="disc-empty">{t('automation.filter.empty')}</div>}
            </div>;
          },
          sidebarFooter: <CollectionSidebarFooter
            label={t('mcp.sidebar.hint')}
            navigateLabel={t('disc.sidebar.navigate')}
            searchLabel={t('disc.sidebar.searchShortcut')}
          />,
          renderDetail: config => (
            config
              ? <PluginDetailPanel cfg={config} state={state} />
              : <div className="collection-shell-detail-empty-hint">{t('mcp.selectHint')}</div>
          ),
          renderEmpty: () => <div className="mcp-filter-empty">{t('automation.filter.empty')}</div>,
        }}
      />
      {showBuiltinFallback && (
        <article
          className="mcp-installed-card mcp-installed-card-static"
          data-kind="mcp"
          data-testid="mcp-kronn-internal-card"
          title={t('mcp.builtin.tooltip')}
        >
          <div className="mcp-plugin-card-header">
            <span className="mcp-plugin-card-icon"><Plug size={16} /></span>
            <div className="mcp-plugin-card-identity">
              <span className="mcp-installed-name">{t('mcp.builtin.tileTitle')}</span>
              <span className="mcp-plugin-card-server">{t('mcp.builtin.tileCat')}</span>
            </div>
            <span className="mcp-origin-badge mcp-origin-official">{t('mcp.builtin.tileBadge')}</span>
          </div>
          <div className="mcp-plugin-card-meta">
            <span className="mcp-scope-badge mcp-scope-global">{t('mcp.scope.globalBadge')}</span>
          </div>
          <div className="mcp-plugin-card-footer">
            <PluginKindBadge kind="mcp" />
          </div>
        </article>
      )}
    </div>
  );
}
