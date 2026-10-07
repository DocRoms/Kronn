import { useState, useEffect, useMemo, useCallback } from 'react';
import type { ToastFn } from '../../hooks/useToast';
import { mcps as mcpsApi, apiCallLogs, type EndpointDrift } from '../../lib/api';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import { usePersistentIdSet } from '../../hooks/usePersistentIdSet';
import { usePersistentSidebarOpen } from '../../hooks/usePersistentSidebarOpen';
import { userError } from '../../lib/userError';
import type { McpConfigDisplay, McpDefinition, McpOverview, McpProbeResponse, McpRescanReport, HostSyncMode, PluginInterface, PluginKind, Project } from '../../types/generated';
import { compactPluginCredentials } from '../../lib/pluginCredentials';
import { hasAgentScope, slugify } from './mcpPageHelpers';
import { configHealth, isAvailableLocally, visibleToPluginProject, type PluginHealthState } from './pluginHealth';

const MCP_COLLAPSED_GROUPS_STORAGE_KEY = 'kronn:mcpCollapsedGroups';

function readCollapsedMcpGroups(): Set<string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(MCP_COLLAPSED_GROUPS_STORAGE_KEY) ?? '[]') as unknown;
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((group): group is string => typeof group === 'string'));
  } catch {
    return new Set();
  }
}

interface UsePluginListStateArgs {
  projects: Project[];
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
  refetchMcps: () => void;
  favoritesReady: boolean;
  initialSelectedConfigId?: string | null;
  /** Owned selection: see `McpPageProps`. */
  selectedConfigId?: string | null;
  onSelectedConfigChange?: (configId: string | null) => void;
  t: (key: string, ...args: (string | number)[]) => string;
  toast: ToastFn;
  isMobile: boolean;
}

/** Plugin list + inline detail panel state: search/sort/filter, sidebar,
 *  selection, drift/probe results, per-config CRUD handlers (scope
 *  toggles, host sync, label rename, secrets edit, context files,
 *  delete) and the derived `visibleConfigs` list. Split out of the
 *  monolithic `useMcpPageState` (KT-830) to stay under the page's
 *  per-file line budget — this is the "liste + fiche" half. */
export function usePluginListState({ projects, mcpOverview, mcpRegistry, refetchMcps, favoritesReady, initialSelectedConfigId, selectedConfigId: ownedConfigId, onSelectedConfigChange, t, toast, isMobile }: UsePluginListStateArgs) {
  const [editingLabelId, setEditingLabelId] = useState<string | null>(null);
  const [editingLabelText, setEditingLabelText] = useState('');

  const [mcpSearch, setMcpSearch] = useState('');
  const [mcpSort, setMcpSort] = useState<'name' | 'kind' | 'scope'>('name');
  const [mcpSortReversed, setMcpSortReversed] = useState(() => {
    try {
      const saved = localStorage.getItem('kronn:mcpSort');
      return saved === 'za';
    } catch { return false; }
  });
  const [mcpKindFilter, setMcpKindFilter] = useState<'all' | 'mcp' | 'api' | 'cli'>('all');
  const [mcpHealthFilter, setMcpHealthFilter] = useState<'all' | PluginHealthState>('all');
  const [mcpSyncFilter, setMcpSyncFilter] = useState<'all' | 'local' | 'none'>('all');
  const [mcpSearchPanel, setMcpSearchPanel] = useState<'filters' | 'sort' | null>(null);
  const [sidebarOpen, setSidebarOpen] = usePersistentSidebarOpen('kronn:plugins:sidebarCollapsed', isMobile);
  const [collapsedMcpGroups, setCollapsedMcpGroups] = useState<Set<string>>(readCollapsedMcpGroups);
  const [selectedConfigIds, setSelectedConfigIds] = useState<Set<string>>(new Set());
  const availableConfigIds = useMemo(() => mcpOverview.configs.map(config => config.id), [mcpOverview.configs]);
  const { ids: favoriteConfigIds, toggle: toggleConfigFavorite } = usePersistentIdSet('kronn:collection-favorites:plugins', availableConfigIds, favoritesReady);
  useEffect(() => {
    try {
      localStorage.setItem('kronn:mcpSort', mcpSortReversed ? 'za' : 'az');
    } catch { /* localStorage disabled (incognito / quota) — sort defaults to A→Z on next load */ }
  }, [mcpSortReversed]);
  useEffect(() => {
    try {
      localStorage.setItem(MCP_COLLAPSED_GROUPS_STORAGE_KEY, JSON.stringify([...collapsedMcpGroups]));
    } catch { /* localStorage may be unavailable in private/restricted browser modes. */ }
  }, [collapsedMcpGroups]);
  const [ownSelectedConfigId, setOwnSelectedConfigId] = useState<string | null>(initialSelectedConfigId ?? null);
  const selectedConfigId = ownedConfigId !== undefined ? ownedConfigId : ownSelectedConfigId;
  const setSelectedConfigId = useCallback((configId: string | null) => {
    setOwnSelectedConfigId(configId);
    onSelectedConfigChange?.(configId);
  }, [onSelectedConfigChange]);
  const [storedProjectId, setSelectedProjectId] = useState(() => {
    try {
      const saved = localStorage.getItem('kronn:mcpSelectedProject') ?? '__all__';
      return saved === '__all__' || saved === '__none__' || projects.some(project => project.id === saved)
        ? saved
        : '__all__';
    }
    catch { return '__all__'; }
  });
  const [syncing, setSyncing] = useState(false);
  const [testingProjectId, setTestingProjectId] = useState<string | null>(null);
  const [rescanPreview, setRescanPreview] = useState<McpRescanReport | null>(null);
  const [portabilityMode, setPortabilityMode] = useState<'export' | 'import' | null>(null);

  // A project deleted since the filter was saved must not leave the list
  // filtered by something the Project select can no longer show. Derived, not
  // reset: `projects` can be momentarily empty while it loads, and the saved
  // choice should survive that.
  const selectedProjectId = storedProjectId === '__all__' || storedProjectId === '__none__'
    || projects.some(project => project.id === storedProjectId)
    ? storedProjectId
    : '__all__';
  useEffect(() => {
    try { localStorage.setItem('kronn:mcpSelectedProject', storedProjectId); }
    catch { /* navigation preference remains in memory */ }
  }, [storedProjectId]);

  // Endpoints that keep failing, grouped by plugin. A spec is written once and
  // never re-checked against the API, so when it drifts nothing says so — this
  // reads the call log Kronn already keeps. Failure to load is not worth an
  // error: the page works without it.
  const [driftBySlug, setDriftBySlug] = useState<Record<string, EndpointDrift[]>>({});
  useEffect(() => {
    let cancelled = false;
    void apiCallLogs.drift()
      .then(rows => {
        if (cancelled) return;
        const grouped: Record<string, EndpointDrift[]> = {};
        for (const row of rows ?? []) {
          (grouped[row.plugin_slug] ??= []).push(row);
        }
        setDriftBySlug(grouped);
      })
      .catch(() => { /* advisory only */ });
    return () => { cancelled = true; };
  }, []);
  const [probeByConfig, setProbeByConfig] = useState<Record<string, McpProbeResponse>>({});
  const [probeTestedAtByConfig, setProbeTestedAtByConfig] = useState<Record<string, string>>({});
  const [probingConfigId, setProbingConfigId] = useState<string | null>(null);

  const handleProbeConfig = useAsyncGuard(async (configId: string) => {
    setProbingConfigId(configId);
    try {
      const result = await mcpsApi.probeConfig(configId);
      setProbeByConfig(previous => ({ ...previous, [configId]: result }));
      setProbeTestedAtByConfig(previous => ({ ...previous, [configId]: new Date().toISOString() }));
      refetchMcps();
    } catch (error) {
      console.warn('Failed to probe plugin:', error);
      toast(t('mcp.probeFailed', userError(error)), 'error');
    } finally {
      setProbingConfigId(null);
    }
  });

  const handleTestProject = useAsyncGuard(async (projectId: string, targets: McpConfigDisplay[]) => {
    setTestingProjectId(projectId);
    try {
      const results = projectId === '__all__'
        ? (await mcpsApi.testAll()).results
        : await Promise.all(targets.map(async config => ({
          config_id: config.id,
          probe: await mcpsApi.probeConfig(config.id),
        })));
      const testedAt = new Date().toISOString();
      setProbeByConfig(previous => ({
        ...previous,
        ...Object.fromEntries(results.map(result => [result.config_id, result.probe])),
      }));
      setProbeTestedAtByConfig(previous => ({
        ...previous,
        ...Object.fromEntries(results.map(result => [result.config_id, testedAt])),
      }));
      refetchMcps();
      toast(t('mcp.projectTestComplete', results.length), 'success');
    } catch (error) {
      console.warn('Failed to test plugin project:', error);
      toast(t('mcp.projectTestFailed', userError(error)), 'error');
    } finally {
      setTestingProjectId(null);
    }
  });

  const handlePreviewRescan = useAsyncGuard(async () => {
    setSyncing(true);
    try {
      setRescanPreview(await mcpsApi.refresh(true));
    } catch (error) {
      console.warn('Failed to preview MCP rescan:', error);
      toast(t('mcp.rescanFailed', userError(error)), 'error');
    } finally {
      setSyncing(false);
    }
  });

  const handleApplyRescan = useAsyncGuard(async () => {
    setSyncing(true);
    try {
      const report = await mcpsApi.refresh(false);
      setRescanPreview(null);
      refetchMcps();
      toast(t(
        'mcp.rescanApplied',
        report.configs_created,
        report.configs_merged,
        report.projects_rewritten ?? 0,
        report.configs_deleted,
      ), 'success');
    } catch (error) {
      console.warn('Failed to apply MCP rescan:', error);
      toast(t('mcp.rescanFailed', userError(error)), 'error');
    } finally {
      setSyncing(false);
    }
  });

  const handleSetPreferredInterface = useAsyncGuard(
    async (configId: string, preferredInterface: PluginInterface) => {
      try {
        await mcpsApi.updateConfig(configId, { preferred_interface: preferredInterface });
        await refetchMcps();
      } catch (e) {
        console.warn('Failed to set preferred plugin interface:', e);
        toast(t('common.actionFailed', userError(e)), 'error');
      }
    },
  );

  const handleSaveLabel = async (configId: string) => {
    if (!editingLabelText.trim()) return;
    try {
      await mcpsApi.updateConfig(configId, { label: editingLabelText.trim() });
      setEditingLabelId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save label:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  // Single-plugin deletion lives only in the detail sheet, behind its two-step
  // confirmation; it returns success so the caller closes the sheet after persistence.
  const handleDeleteMcpConfig = async (configId: string): Promise<boolean> => {
    const cfg = mcpOverview.configs.find(c => c.id === configId);
    const label = cfg?.label ?? configId;
    try {
      await mcpsApi.deleteConfig(configId);
      refetchMcps();
      toast(t('mcp.deleteConfigSuccess', label), 'success');
      return true;
    } catch (e) {
      console.warn('Failed to delete MCP config:', e);
      toast(t('mcp.deleteConfigError', userError(e)), 'error');
      return false;
    }
  };

  const handleDeleteSelectedMcpConfigs = async (selected: McpConfigDisplay[]) => {
    if (selected.length === 0 || !confirm(t('collection.deleteConfirm', selected.length))) return;
    try {
      await Promise.all(selected.map(config => mcpsApi.deleteConfig(config.id)));
      if (selectedConfigId && selected.some(config => config.id === selectedConfigId)) setSelectedConfigId(null);
      refetchMcps();
      toast(t('collection.deleteSuccess', selected.length), 'success');
    } catch (cause) {
      toast(t('collection.deleteError', userError(cause)), 'error');
      throw cause;
    }
  };

  const handleToggleConfigGlobal = async (config: McpConfigDisplay) => {
    const nextGlobal = !config.is_global;
    if (!hasAgentScope(nextGlobal, config.include_general, config.project_ids)) {
      toast(t('mcp.scopeRequired'), 'warning');
      return;
    }
    try {
      await mcpsApi.updateConfig(config.id, { is_global: nextGlobal });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle global:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  const handleToggleConfigGeneral = async (config: McpConfigDisplay) => {
    const nextGeneral = !config.include_general;
    if (!hasAgentScope(config.is_global, nextGeneral, config.project_ids)) {
      toast(t('mcp.scopeRequired'), 'warning');
      return;
    }
    try {
      await mcpsApi.updateConfig(config.id, { include_general: nextGeneral });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle general scope:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  /** Update host_sync (CLI scope: None/GlobalOnly/MirrorAll). UX#2 — single
   *  source of edit; the SettingsPage section delegates here via deeplink. */
  const handleSetHostSync = async (configId: string, mode: HostSyncMode) => {
    try {
      await mcpsApi.updateConfig(configId, { host_sync: mode });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to set host_sync:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  const handleToggleConfigProject = async (configId: string, projectId: string, currentlyLinked: boolean) => {
    const config = mcpOverview.configs.find(c => c.id === configId);
    if (!config) return;
    const newIds = currentlyLinked
      ? config.project_ids.filter(id => id !== projectId)
      : [...config.project_ids, projectId];
    if (!hasAgentScope(config.is_global, config.include_general, newIds)) {
      toast(t('mcp.scopeRequired'), 'warning');
      return;
    }
    try {
      await mcpsApi.setConfigProjects(configId, { project_ids: newIds });
      refetchMcps();
    } catch (e) {
      console.warn('Failed to toggle project:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    }
  };

  // Edit secrets (registry + view-only custom plugin env)
  const [editingEnvId, setEditingEnvId] = useState<string | null>(null);
  const [editingEnv, setEditingEnv] = useState<Record<string, string>>({});
  const [editingEnvLoading, setEditingEnvLoading] = useState(false);
  const [visibleFields, setVisibleFields] = useState<Set<string>>(new Set());
  const [editingEnvError, setEditingEnvError] = useState<string | null>(null);

  const handleStartEditSecrets = async (configId: string): Promise<boolean> => {
    if (editingEnvId === configId) { setEditingEnvId(null); return false; }
    setEditingEnvLoading(true);
    setVisibleFields(new Set());
    setEditingEnvError(null);
    try {
      const entries = await mcpsApi.revealSecrets(configId);
      const env: Record<string, string> = {};
      entries.forEach(e => { env[e.key] = e.masked_value; });
      setEditingEnv(env);
      setEditingEnvId(configId);
      return true;
    } catch (e) {
      console.warn('Failed to load secrets:', e);
      // Enter edit mode with empty values so the user can re-enter tokens
      const cfg = mcpOverview.configs.find(c => c.id === configId);
      const env: Record<string, string> = {};
      cfg?.env_keys.forEach(k => { env[k] = ''; });
      setEditingEnv(env);
      setEditingEnvId(configId);
      setEditingEnvError(t('mcp.revealWarning'));
      return true;
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const handleSaveSecrets = async () => {
    if (!editingEnvId) return;
    setEditingEnvLoading(true);
    try {
      // 0.8.6 — For Custom plugins, filter the env on save to ONLY the
      // env_keys the current spec declares. Orphans from a prior
      // rename get dropped here (the PATCH replaces the env wholesale,
      // so what we don't send disappears). For registry plugins, the
      // spec is immutable and stored env always matches → no-op.
      const cfg = mcpOverview.configs.find(c => c.id === editingEnvId);
      const server = cfg ? mcpOverview.servers.find(s => s.id === cfg.server_id) : null;
      const specKeys = server?.api_spec?.config_keys?.map(ck => ck.env_key) ?? [];
      const isCustom = cfg?.server_id.startsWith('custom-') ?? false;
      const registryDefinition = cfg
        ? mcpRegistry.find(definition => definition.id === cfg.server_id)
        : undefined;
      const envToSend: Record<string, string> = isCustom && specKeys.length > 0
        ? Object.fromEntries(Object.entries(editingEnv).filter(([k]) => specKeys.includes(k)))
        : compactPluginCredentials(registryDefinition, editingEnv);
      await mcpsApi.updateConfig(editingEnvId, { env: envToSend });
      setEditingEnvId(null);
      refetchMcps();
    } catch (e) {
      console.warn('Failed to save secrets:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    } finally {
      setEditingEnvLoading(false);
    }
  };

  const toggleFieldVisibility = (key: string) => {
    setVisibleFields(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  };

  // MCP context editor
  const [contextEditor, setContextEditor] = useState<{ projectId: string; projectName: string; slug: string; content: string } | null>(null);
  const [contextSaving, setContextSaving] = useState(false);

  const handleOpenContext = async (projectId: string, projectName: string, configLabel: string) => {
    // Slugify the label (same algo as backend)
    const slug = slugify(configLabel);
    try {
      const entry = await mcpsApi.getContext(projectId, slug);
      setContextEditor({ projectId, projectName, slug, content: entry.content });
    } catch {
      // File might not exist yet — create with empty marker
      setContextEditor({ projectId, projectName, slug, content: `# ${configLabel} — Usage Context\n\n> Instructions for AI agents using **${configLabel}** in this project.\n> Edit this file with project-specific rules.\n\n## Rules\n\n` });
    }
  };

  const handleSaveContext = async () => {
    if (!contextEditor) return;
    setContextSaving(true);
    try {
      await mcpsApi.updateContext(contextEditor.projectId, contextEditor.slug, contextEditor.content);
      setContextEditor(null);
    } catch (e) {
      console.warn('Failed to save context:', e);
      toast(t('common.actionFailed', userError(e)), 'error');
    } finally {
      setContextSaving(false);
    }
  };

  // ── Computed ──

  const { servers, configs } = mcpOverview;
  const totalConfigs = configs.length;
  const globalConfigs = configs.filter(c => c.is_global);
  const isBuiltinConfig = (config: McpConfigDisplay) => (
    config.server_name.toLowerCase() === 'kronn-internal'
    || config.server_id === 'kronn-internal'
    || config.server_id.endsWith(':kronn-internal')
  );
  const builtinConfig = configs.find(isBuiltinConfig);

  const healthFor = (cfg: McpConfigDisplay) => configHealth(cfg, {
    liveProbe: probeByConfig[cfg.id],
    liveTestedAt: probeTestedAtByConfig[cfg.id],
    incomplete: mcpOverview.incomplete_configs.find(item => item.config_id === cfg.id),
    hasEndpointDrift: (driftBySlug[cfg.server_id]?.length ?? 0) > 0,
  });
  const activeFilterCount = [
    mcpKindFilter !== 'all', selectedProjectId !== '__all__', mcpHealthFilter !== 'all', mcpSyncFilter !== 'all',
  ].filter(Boolean).length;
  const clearPluginFilters = () => {
    setMcpKindFilter('all');
    setSelectedProjectId('__all__');
    setMcpHealthFilter('all');
    setMcpSyncFilter('all');
  };

  // KT-828 — the badge/filter classification is computed once server-side
  // (`effective_kind`/`interfaces`) so it can't drift from what the agent
  // actually uses. The API/MCP filters read `interfaces` rather than
  // `effective_kind` directly so a CLI-first plugin that ALSO exposes an
  // API or MCP surface (Fastly: Cli + [Api, Mcp, Cli]) still matches those
  // filters — only the CLI filter is gated on the `cli` kind itself.
  const visibleConfigs = [...configs]
    .filter(cfg => {
      const matchesKind = mcpKindFilter === 'all'
        || (mcpKindFilter === 'mcp' && cfg.interfaces.includes('mcp'))
        || (mcpKindFilter === 'api' && cfg.interfaces.includes('api'))
        || (mcpKindFilter === 'cli' && cfg.effective_kind === 'cli');
      const matchesSync = mcpSyncFilter === 'all' || isAvailableLocally(cfg) === (mcpSyncFilter === 'local');
      return matchesKind
        && visibleToPluginProject(cfg, selectedProjectId)
        && matchesSync
        && (mcpHealthFilter === 'all' || healthFor(cfg) === mcpHealthFilter);
    })
    .sort((a, b) => {
      const aKind = a.effective_kind;
      const bKind = b.effective_kind;
      const byName = a.label.localeCompare(b.label, undefined, {
        sensitivity: 'base',
        numeric: true,
      });
      let result: number;
      if (mcpSort === 'kind') {
        const order: Record<PluginKind, number> = {
          mcp: 0,
          hybrid: 1,
          api: 2,
          cli: 3,
        };
        result = order[aKind] - order[bKind] || byName;
      } else if (mcpSort === 'scope') {
        const aScope = a.is_global ? Number.MAX_SAFE_INTEGER : a.project_ids.length;
        const bScope = b.is_global ? Number.MAX_SAFE_INTEGER : b.project_ids.length;
        result = bScope - aScope || byName;
      } else {
        result = byName;
      }
      return mcpSortReversed ? -result : result;
    });

  const pluginSearchLabel = (config: McpConfigDisplay) => {
    const visibleProjectNames = projects
      .filter(project => visibleToPluginProject(config, project.id))
      .map(project => project.name);
    const noProject = visibleToPluginProject(config, '__none__') ? t('disc.noProject') : '';
    return `${config.label} ${config.server_name} ${visibleProjectNames.join(' ')} ${noProject}`;
  };
  const query = mcpSearch.trim().toLocaleLowerCase();
  const matchingConfigs = visibleConfigs.filter(config => (
    !query || pluginSearchLabel(config).toLocaleLowerCase().includes(query)
  ));

  // The built-in card is not a scoped config: any project / health / sync
  // filter is a question it cannot answer, so it steps aside.
  const builtinMatchesList = (mcpKindFilter === 'all' || mcpKindFilter === 'mcp')
    && selectedProjectId === '__all__' && mcpHealthFilter === 'all' && mcpSyncFilter === 'all'
    && (!mcpSearch || t('mcp.builtin.tileTitle').toLowerCase().includes(mcpSearch.toLowerCase()));

  useEffect(() => {
    const selectedMatchesQuery = matchingConfigs.some(config => config.id === selectedConfigId);
    if (selectedConfigId && !selectedMatchesQuery) {
      setSelectedConfigId(null);
    }
  }, [matchingConfigs, selectedConfigId, setSelectedConfigId]);

  return {
    editingLabelId, setEditingLabelId, editingLabelText, setEditingLabelText, handleSaveLabel,

    mcpSearch, setMcpSearch, mcpSort, setMcpSort, mcpSortReversed, setMcpSortReversed,
    mcpKindFilter, setMcpKindFilter, mcpHealthFilter, setMcpHealthFilter, mcpSyncFilter, setMcpSyncFilter,
    activeFilterCount, clearPluginFilters, mcpSearchPanel, setMcpSearchPanel,
    sidebarOpen, setSidebarOpen, collapsedMcpGroups, setCollapsedMcpGroups,
    selectedConfigIds, setSelectedConfigIds, favoriteConfigIds, toggleConfigFavorite,
    selectedConfigId, setSelectedConfigId, selectedProjectId, setSelectedProjectId,
    syncing, setSyncing, testingProjectId, rescanPreview, setRescanPreview,
    handleTestProject, handlePreviewRescan, handleApplyRescan,
    portabilityMode, setPortabilityMode,

    driftBySlug, probeByConfig, probeTestedAtByConfig, probingConfigId, handleProbeConfig, handleSetPreferredInterface,

    handleDeleteMcpConfig, handleDeleteSelectedMcpConfigs,
    handleToggleConfigGlobal, handleToggleConfigGeneral, handleSetHostSync, handleToggleConfigProject,

    editingEnvId, setEditingEnvId, editingEnv, setEditingEnv, editingEnvLoading, visibleFields, setVisibleFields, editingEnvError,
    handleStartEditSecrets, handleSaveSecrets, toggleFieldVisibility,

    contextEditor, setContextEditor, contextSaving, handleOpenContext, handleSaveContext,

    servers, configs, totalConfigs, globalConfigs, isBuiltinConfig, builtinConfig, builtinMatchesList,
    visibleConfigs, matchingConfigs, pluginSearchLabel, healthFor,
  };
}
