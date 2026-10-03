import { ChevronRight, Clock, Plus, Puzzle, Trash2, CheckSquare, Plug, Key } from 'lucide-react';
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
import { PluginProjectOverview } from './PluginProjectOverview';
import { PluginToolbarPanel, PluginToolbarToggle } from './PluginToolbar';
import { latestPluginTest } from './pluginHealth';
import type { McpPageState } from './useMcpPageState';

export function PluginCollectionView({ state }: { state: McpPageState }) {
  const {
    t, isMobile,
    totalConfigs, visibleConfigs, favoriteConfigIds, toggleConfigFavorite,
    mcpSearch, setMcpSearch, selectedConfigId, setSelectedConfigId,
    selectedConfigIds, setSelectedConfigIds, handleDeleteSelectedMcpConfigs,
    sidebarOpen, setSidebarOpen,
    isBuiltinConfig, driftBySlug, healthFor,
    collapsedMcpGroups, setCollapsedMcpGroups,
    handleDeleteMcpConfig, showBuiltinFallback,
    activeFilterCount, clearPluginFilters, pluginSearchLabel,
    setShowAddMcp, setAddMcpSelected, setAddMcpSearch, addMcpTriggerRef,
  } = state;

  return <div className="mcp-plugin-shell">
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
      getLabel={pluginSearchLabel}
      isFavorite={config => favoriteConfigIds.has(config.id)}
      onToggleFavorite={config => toggleConfigFavorite(config.id)}
      persistence={{ query: mcpSearch, onQueryChange: setMcpSearch, favoritesOnly: false, onFavoritesOnlyChange: () => {} }}
      selectedId={selectedConfigId}
      onSelect={setSelectedConfigId}
      selectedIds={selectedConfigIds}
      onSelectedIdsChange={setSelectedConfigIds}
      actions={[{
        id: 'delete',
        label: t('collection.deleteSelected'),
        icon: <Trash2 size={15} />,
        danger: true,
        disabled: selected => selected.length === 0,
        onSelect: handleDeleteSelectedMcpConfigs,
      }]}
      isMobile={isMobile}
      sidebarOpen={sidebarOpen}
      onSidebarOpenChange={setSidebarOpen}
      globalSearchShortcut
      showSearchClear
      showControls={false}
      labels={{
        search: t('mcp.search'), favorites: t('collection.favorites'), clearFilters: t('collection.clearFilters'),
        moreActions: t('collection.moreActions'), openCollection: t('collection.openCollection'), closeCollection: t('collection.closeCollection'),
        selectItem: t('collection.selectItem'), selectMultiple: t('collection.selectMultiple'), cancelSelection: t('collection.cancelSelection'),
        selectedCount: count => t('collection.selectedCount', count),
      }}
      slots={{
        afterSidebarHeader: <PluginToolbarPanel state={state} />,
        sidebarHeaderEnd: <PluginToolbarToggle state={state} />,
        renderList: ({ visibleItems, getRowProps, canMultiSelect, isMultiSelected, toggleMultiSelection }) => {
          const collapsedGroups = canMultiSelect || mcpSearch.trim() ? new Set<string>() : collapsedMcpGroups;
          const toggleGroup = (group: string) => setCollapsedMcpGroups(current => {
            const next = new Set(current);
            if (next.has(group)) next.delete(group); else next.add(group);
            return next;
          });
          const row = (config: McpConfigDisplay, keyPrefix: string) => {
            const rowProps = getRowProps(config);
            const selected = isMultiSelected(config);
            const stateValue = healthFor(config);
            const scopeLabels = [
              config.is_global ? t('mcp.globalAll') : null,
              config.include_general ? t('disc.general') : null,
              config.project_ids.length > 0
                ? `${config.project_ids.length} ${config.project_ids.length > 1 ? t('mcp.projectPlural') : t('mcp.project')}`
                : null,
            ].filter((label): label is string => Boolean(label));
            const worstDrift = driftBySlug[config.server_id]?.[0];
            return <div className="disc-swipe-wrap" key={`${keyPrefix}-${config.id}`}>
              <div
                className="disc-item mcp-sidebar-plugin-row"
                data-active={config.id === selectedConfigId}
                data-selected={selected}
                data-kind={config.effective_kind}
                data-config-id={config.id}
                data-testid={isBuiltinConfig(config) ? 'mcp-kronn-internal-card' : undefined}
              >
                <button
                  type="button"
                  {...rowProps}
                  className={`${rowProps.className} disc-item-open`}
                  onClick={canMultiSelect
                    ? () => toggleMultiSelection(config.id)
                    : rowProps.onClick}
                  aria-label={canMultiSelect ? `${config.label} · ${t('collection.selectItem')}` : `${config.label} — ${t('mcp.openDetails')}`}
                  role={canMultiSelect ? 'checkbox' : undefined}
                  aria-checked={canMultiSelect ? selected : undefined}
                >
                  {canMultiSelect && <span className="disc-item-selection-box" data-selected={selected} aria-hidden="true">{selected && <CheckSquare size={12} />}</span>}
                  <span className="mcp-health-dot" data-state={stateValue} aria-label={t(`mcp.health.${stateValue}`)} />
                  <span className="disc-item-content">
                    <span className="disc-item-title">
                      <span className="disc-item-title-text">{config.label}</span>
                      {isBuiltinConfig(config) && <span className="mcp-origin-badge mcp-origin-official">{t('mcp.builtin.tileBadge')}</span>}
                    </span>
                    <span className="disc-item-meta">
                      <span className="disc-item-meta-summary">
                        {config.server_name !== config.label ? `${config.server_name} · ` : ''}
                        {scopeLabels.length > 0 ? scopeLabels.join(' · ') : t('mcp.scopeOrphanShort')}
                      </span>
                      <PluginKindBadge kind={config.effective_kind} />
                      {config.effective_kind !== 'api' && <HostSyncChip mode={config.host_sync} />}
                      {config.env_keys.length > 0 && <span className="mcp-installed-keys"><Key size={9} /> {config.env_keys.length}</span>}
                      {config.secrets_broken && <span className="mcp-scope-badge mcp-scope-broken" title={t('mcp.secretsBroken')}>⚠ {t('mcp.secretsBrokenShort')}</span>}
                      {config.registry_drift && <span className="mcp-scope-badge mcp-scope-drift" title={t('mcp.registryDrift')}>⚠ {t(config.registry_drift.orphaned ? 'mcp.registryOrphanShort' : 'mcp.registryDriftShort')}</span>}
                      {worstDrift && <span className="mcp-scope-badge mcp-scope-drift" data-testid={`mcp-endpoint-drift-${config.server_id}`} title={t(worstDrift.successes > 0 ? 'mcp.drift.sometimes' : 'mcp.drift.never', worstDrift.failures, worstDrift.endpoint_path, `HTTP ${worstDrift.http_status}`)}>⚠ {t('mcp.drift.short')}</span>}
                    </span>
                  </span>
                </button>
                {!canMultiSelect && <CollectionRowActions
                  itemName={config.label}
                  favorite={{ active: favoriteConfigIds.has(config.id), onToggle: () => toggleConfigFavorite(config.id), activeLabel: t('disc.unpin'), inactiveLabel: t('disc.pin') }}
                  menuLabel={t('collection.moreActions')}
                  copyId={config.id}
                  copyLabel={t('disc.copyId')}
                  actions={[{ id: 'delete', label: t('mcp.deleteConfig'), icon: <Trash2 size={12} />, danger: true, onSelect: async () => { await handleDeleteMcpConfig(config.id); } }]}
                />}
              </div>
            </div>;
          };

          const favorites = canMultiSelect ? [] : visibleItems.filter(config => favoriteConfigIds.has(config.id));
          const recent = canMultiSelect ? [] : visibleItems
            .filter(config => !favoriteConfigIds.has(config.id))
            .map(config => ({ config, testedAt: latestPluginTest(config) }))
            .filter((item): item is { config: McpConfigDisplay; testedAt: string } => item.testedAt !== null)
            .sort((left, right) => right.testedAt.localeCompare(left.testedAt))
            .slice(0, 5)
            .map(item => item.config);
          const favoritesCollapsed = collapsedGroups.has('favorites');
          const recentCollapsed = collapsedGroups.has('recent');
          const allCollapsed = collapsedGroups.has('all');
          // A plugin is listed once (KT-907): the shortcut sections take theirs
          // out of the full list, like the Discussions sidebar does for favorites.
          const shortcutIds = new Set([...favorites, ...recent].map(config => config.id));
          const others = visibleItems.filter(config => !shortcutIds.has(config.id));

          return <div className="disc-sidebar-list mcp-sidebar-items">
            {favorites.length > 0 && <div className="disc-sidebar-section disc-sidebar-favorites" data-expanded={!favoritesCollapsed}>
              <CollectionFavoritesHeader label={t('disc.favorites')} count={favorites.length} expanded={!favoritesCollapsed} onToggle={() => toggleGroup('favorites')} />
              {!favoritesCollapsed && favorites.map(config => row(config, 'favorite'))}
            </div>}
            {recent.length > 0 && <div className="disc-sidebar-section mcp-sidebar-recent" data-expanded={!recentCollapsed}>
              <button type="button" className="disc-group-btn" data-no-border="true" onClick={() => toggleGroup('recent')} aria-expanded={!recentCollapsed}>
                <Clock size={10} /><span>{t('mcp.recentlyTested')}</span><span className="disc-group-count">{recent.length}</span>
              </button>
              {!recentCollapsed && recent.map(config => row(config, 'recent'))}
            </div>}
            {others.length > 0 && <div className="disc-sidebar-section mcp-sidebar-all" data-expanded={!allCollapsed}>
              <button type="button" className="disc-group-btn" data-no-border="true" onClick={() => toggleGroup('all')} aria-expanded={!allCollapsed}>
                <ChevronRight size={10} className="disc-chevron" data-expanded={!allCollapsed} />
                <span>{t('mcp.allPlugins')}</span><span className="disc-group-count">{others.length}</span>
              </button>
              {!allCollapsed && others.map(config => row(config, 'all'))}
            </div>}
            {visibleItems.length === 0 && <div className="disc-empty">
              {t('mcp.filter.empty')}
              {activeFilterCount > 0 && <button type="button" className="mcp-filter-clear" onClick={clearPluginFilters}>{t('collection.clearFilters')}</button>}
            </div>}
          </div>;
        },
        sidebarFooter: <CollectionSidebarFooter label={t('mcp.sidebar.hint')} navigateLabel={t('disc.sidebar.navigate')} searchLabel={t('disc.sidebar.searchShortcut')} />,
        renderDetail: config => <div className="mcp-project-detail-layout" data-sheet-open={Boolean(config)}>
          <PluginProjectOverview state={state} />
          {config && <PluginDetailPanel key={config.id} cfg={config} state={state} />}
        </div>,
        renderEmpty: () => <div className="mcp-filter-empty">{t('mcp.filter.empty')}</div>,
      }}
    />
    {showBuiltinFallback && <article className="mcp-installed-card mcp-installed-card-static" data-kind="mcp" data-testid="mcp-kronn-internal-card" title={t('mcp.builtin.tooltip')}>
      <div className="mcp-plugin-card-header">
        <span className="mcp-plugin-card-icon"><Plug size={16} /></span>
        <div className="mcp-plugin-card-identity"><span className="mcp-installed-name">{t('mcp.builtin.tileTitle')}</span><span className="mcp-plugin-card-server">{t('mcp.builtin.tileCat')}</span></div>
        <span className="mcp-origin-badge mcp-origin-official">{t('mcp.builtin.tileBadge')}</span>
      </div>
    </article>}
  </div>;
}
