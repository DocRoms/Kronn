import { useCallback, useEffect, useRef } from 'react';
import { mcps as mcpsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { useIsMobile } from '../../hooks/useMediaQuery';
import { useToast } from '../../hooks/useToast';
import type { ToastFn } from '../../hooks/useToast';
import { userError } from '../../lib/userError';
import type { AgentType, McpDefinition, McpOverview, Project } from '../../types/generated';
import { compactPluginCredentials } from '../../lib/pluginCredentials';
import { usePluginListState } from './usePluginListState';
import { useAddPluginRegistryState } from './useAddPluginRegistryState';
import { useCustomApiFormState } from './useCustomApiFormState';
import { visibleToPluginProject } from './pluginHealth';

export interface McpPageProps {
  projects: Project[];
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
  refetchMcps: () => void;
  /** False until the overview request has completed; protects restored
   * favorites from its initial empty fallback. */
  favoritesReady?: boolean;
  initialSelectedConfigId?: string | null;
  /** Installed agent types — threaded through to the Custom API AI
   *  helper bubble so the user can pick which local agent runs the
   *  helper conversation. Optional: when empty, the helper trigger
   *  surfaces a "no agents installed" message instead of opening. */
  installedAgentTypes?: AgentType[];
  /** Backend output language (Settings → Output language) — used as the
   *  agent's reply language inside the helper. Falls back to 'fr' when
   *  missing, mirroring the Dashboard-level default. */
  configLanguage?: string;
}

/** Shared state, computed values and handlers behind the Plugins page.
 *  Every subcomponent under `components/plugins/` receives this object
 *  as its `state` prop rather than a hand-picked prop list — the page
 *  was a single closure before the KT-830 split, and this keeps every
 *  piece reachable exactly like before. Internally this composes 4
 *  smaller hooks (list/detail, add-registry, custom-form)
 *  kept under the page's per-file line budget; this file only owns the
 *  handful of handlers that genuinely cross those boundaries
 *  (`resetAddMcp`, `handleAddMcpFromRegistry`, the Escape-key effect). */
export type McpPageState =
  & ReturnType<typeof usePluginListState>
  & ReturnType<typeof useAddPluginRegistryState>
  & ReturnType<typeof useCustomApiFormState>
  & {
    t: (key: string, ...args: (string | number)[]) => string;
    isMobile: boolean;
    toast: ToastFn;
    ToastContainer: () => React.ReactElement | null;
    projects: Project[];
    mcpOverview: McpOverview;
    mcpRegistry: McpDefinition[];
    refetchMcps: () => void;
    installedAgentTypes?: AgentType[];
    configLanguage?: string;
    resetAddMcp: () => void;
    handleAddMcpModalKeyDown: (event: React.KeyboardEvent<HTMLDivElement>) => void;
    handleAddMcpFromRegistry: () => Promise<void>;
    showBuiltinFallback: boolean;
  };

export function useMcpPageState({ projects, mcpOverview, mcpRegistry, refetchMcps, favoritesReady = true, initialSelectedConfigId, installedAgentTypes, configLanguage }: McpPageProps): McpPageState {
  const { t } = useT();
  const isMobile = useIsMobile();
  const { toast, ToastContainer } = useToast();

  const list = usePluginListState({ projects, mcpOverview, mcpRegistry, refetchMcps, favoritesReady, initialSelectedConfigId, t, toast, isMobile });
  const addRegistry = useAddPluginRegistryState({ mcpOverview, mcpRegistry });
  const customForm = useCustomApiFormState();

  const {
    setShowAddMcp, setAddMcpSelected, setAddMcpLabel, setAddMcpEnv,
    setAddMcpGlobal, setAddMcpProjectIds, setAddMcpIncludeGeneral, setAddMcpHostSync, setAddMcpSearch, addMcpRef,
  } = addRegistry;
  const {
    setCustomName, setCustomBaseUrl, setCustomDescription, setCustomDocsUrl,
    setCustomFields, setCustomEndpoints, setEditingCustomServerId,
    setEditingCustomConfigId, setEditingCustomOriginalScope, setCustomAuth, setReplacingFields,
  } = customForm;

  const resetAddMcp = useCallback(() => {
    setShowAddMcp(false);
    setAddMcpSelected(null);
    setAddMcpLabel('');
    setAddMcpEnv({});
    setAddMcpGlobal(false);
    setAddMcpProjectIds([]);
    setAddMcpIncludeGeneral(true);
    setAddMcpHostSync(false);
    setAddMcpSearch('');
    setCustomName('');
    setCustomBaseUrl('');
    setCustomDescription('');
    setCustomDocsUrl('');
    setCustomFields([{ label: '', value: '' }]);
    setCustomEndpoints([]);
    setEditingCustomServerId(null);
    setEditingCustomConfigId(null);
    setEditingCustomOriginalScope(null);
    setCustomAuth('None');
    setReplacingFields(new Set());
  }, [
    setShowAddMcp, setAddMcpSelected, setAddMcpLabel, setAddMcpEnv, setAddMcpGlobal, setAddMcpProjectIds,
    setAddMcpIncludeGeneral, setAddMcpHostSync, setAddMcpSearch, setCustomName, setCustomBaseUrl, setCustomDescription,
    setCustomDocsUrl, setCustomFields, setCustomEndpoints, setEditingCustomServerId,
    setEditingCustomConfigId, setEditingCustomOriginalScope, setCustomAuth, setReplacingFields,
  ]);
  const handleAddMcpModalKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      resetAddMcp();
      return;
    }
    if (event.key !== 'Tab') return;
    const focusable = [...(addMcpRef.current?.querySelectorAll<HTMLElement>(
      'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])',
    ) ?? [])];
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  // KT-831 — "open the fiche of what I just added". `refetchMcps` isn't
  // awaitable (`() => void`) and the freshly created config isn't in
  // `mcpOverview.configs` on the render right after `createConfig`
  // resolves — selecting it immediately would race the existing
  // search/no-longer-in-list deselect guard in `usePluginListState`
  // (empty selectedConfig lookup ⇒ instant `setSelectedConfigId(null)`).
  // Defer the selection until the id actually shows up post-refetch.
  const pendingSelectConfigIdRef = useRef<string | null>(null);
  const {
    selectedConfigId, setSelectedConfigId, setMcpSearch, setMcpKindFilter,
    setMcpHealthFilter, setMcpSyncFilter, selectedProjectId, setSelectedProjectId,
  } = list;
  useEffect(() => {
    const pendingId = pendingSelectConfigIdRef.current;
    const added = mcpOverview.configs.find(c => c.id === pendingId);
    if (!pendingId || !added) return;
    pendingSelectConfigIdRef.current = null;
    queueMicrotask(() => {
      setMcpSearch('');
      setMcpKindFilter('all');
      setMcpHealthFilter('all');
      setMcpSyncFilter('all');
      // Keep the project the operator is working in, unless the new plugin
      // is outside it and would be filtered out of the list straight away.
      if (!visibleToPluginProject(added, selectedProjectId)) setSelectedProjectId('__all__');
      setSelectedConfigId(pendingId);
    });
  }, [mcpOverview.configs, selectedProjectId, setMcpKindFilter, setMcpHealthFilter, setMcpSyncFilter, setSelectedProjectId, setMcpSearch, setSelectedConfigId]);

  const handleAddMcpFromRegistry = async () => {
    const { addMcpSelected, addMcpLabel, addMcpEnv, addMcpGlobal, addMcpProjectIds, addMcpHostSync } = addRegistry;
    const {
      customName, customBaseUrl, customDescription, customDocsUrl, customFields, customEndpoints, customAuth,
      editingCustomServerId, editingCustomConfigId, editingCustomOriginalScope,
    } = customForm;
    // Refonte 2b (2026-06-10) — the EDIT path no longer rides on the Add
    // panel (`addMcpSelected` stays null while editing in the plugin
    // modal), so route on `editingCustomServerId` as well.
    if (!addMcpSelected && !editingCustomServerId) return;
    // Custom API branch: forward the freeform form as `custom_spec` instead
    // of env-keys. Validation mirrors the backend (name + base_url required)
    // so the user sees the error before the round-trip.
    if (addMcpSelected === 'api-custom' || editingCustomServerId) {
      if (!customName.trim()) {
        toast(t('mcp.custom.errorName'), 'error');
        return;
      }
      if (!customBaseUrl.trim()) {
        toast(t('mcp.custom.errorBaseUrl'), 'error');
        return;
      }
      // 0.8.6 — Edit-existing branch. The form is reused for both
      // create (POST /api/mcps/configs) and edit (PUT /api/mcps/custom/:id).
      // The Edit button on a custom plugin row sets `editingCustomServerId`
      // and pre-fills the form. On submit, we route to the right endpoint.
      // The encrypted env per-config is NOT touched in edit mode — the
      // user uses the existing "edit env" drawer for that.
      if (editingCustomServerId) {
        // 0.8.6 fix 2026-05-20 : capture the name BEFORE `resetAddMcp`
        // clears `customName` to '' — otherwise the success toast read
        // an empty name post-reset and rendered `API «  » mise à jour`
        // (visually nothing). Same defensive capture for the create
        // path below, in case future refactors reorder.
        const savedName = customName.trim();
        const filteredFields = customFields.filter(f => f.label.trim() !== '');
        try {
          // Step 1 — update the spec (name / base_url / docs_url /
          // fields[].label / endpoints / auth). Server row touched here.
          const updateResp = await mcpsApi.updateCustomSpec(editingCustomServerId, {
            name: savedName,
            base_url: customBaseUrl.trim(),
            description: customDescription.trim(),
            docs_url: customDocsUrl.trim() || null,
            fields: filteredFields,
            endpoints: customEndpoints.filter(e => e.path.trim() !== ''),
            auth: customAuth,
          });
          // 0.8.6 (#60) — detect orphan env keys left behind by a rename
          // / removal across all OTHER configs of this server (the
          // current config's env gets wholesale-replaced in step 2 so
          // its orphans clean up automatically, but multi-project configs
          // need an explicit cleanup pass).
          const orphanKeys = updateResp.orphan_env_keys ?? [];
          // Step 2 — patch the encrypted env so "Modifier le plugin" is the
          // SINGLE edit surface for BOTH structure and credentials (the card
          // is read-only by design — we don't want N places to edit one key).
          //
          // 2026-06-09 fix : the value fields are pre-filled EMPTY (not with
          // a masked secret — see openEditCustomPlugin). So `f.value !== ''`
          // now cleanly means "the user typed a NEW value to replace this
          // key"; an untouched (empty) field is SKIPPED, keeping the stored
          // secret. This is what kills the desync bug: the old masked
          // pre-fill made every field look non-empty, so a real key change
          // couldn't be told apart from "leave as-is" and never persisted.
          if (editingCustomConfigId) {
            const newEnv: Record<string, string> = {};
            for (const f of filteredFields) {
              if (f.value !== '') {
                newEnv[customForm.slugEnvKey(f.label)] = f.value;
              }
            }
            // Only PATCH when the user actually entered at least one new
            // value — an empty map would be a no-op round-trip.
            if (Object.keys(newEnv).length > 0) {
              try {
                await mcpsApi.updateConfig(editingCustomConfigId, { env: newEnv });
              } catch (envErr) {
                console.warn('Spec saved but env PATCH failed:', envErr);
                toast(t('mcp.custom.specSavedEnvFailed', userError(envErr)), 'error');
                resetAddMcp();
                refetchMcps();
                return;
              }
            }
          }
          // KT-831 — the scope block (Global / Général / projets) now lives
          // in this same form when editing a Custom API. It used to be
          // silently dropped here: `addMcpGlobal` was never initialized
          // from `cfg.is_global` on open (always read back as `false`) and
          // never sent on save (`updateCustomSpec` has no scope fields).
          // Only PATCH when something actually changed from the values the
          // form opened with — an untouched scope must stay a no-op, same
          // contract as the env PATCH above.
          if (editingCustomConfigId && editingCustomOriginalScope) {
            const scopeChanged = editingCustomOriginalScope.isGlobal !== addMcpGlobal
              || editingCustomOriginalScope.includeGeneral !== addRegistry.addMcpIncludeGeneral
              || editingCustomOriginalScope.projectIds.length !== addMcpProjectIds.length
              || editingCustomOriginalScope.projectIds.some(id => !addMcpProjectIds.includes(id));
            if (scopeChanged) {
              try {
                await mcpsApi.updateConfig(editingCustomConfigId, {
                  is_global: addMcpGlobal,
                  include_general: addRegistry.addMcpIncludeGeneral,
                });
                await mcpsApi.setConfigProjects(editingCustomConfigId, { project_ids: addMcpProjectIds });
              } catch (scopeErr) {
                console.warn('Spec saved but scope PATCH failed:', scopeErr);
                toast(t('common.actionFailed', userError(scopeErr)), 'error');
                resetAddMcp();
                refetchMcps();
                return;
              }
            }
          }
          toast(t('mcp.custom.updated', savedName), 'success');
          // 0.8.6 (#60) — surface orphan-env warning AFTER the success
          // toast so the success path stays visible. The cleanup button
          // lives in the toast itself; if the user dismisses we keep
          // the warning visible until next render (or they reopen the
          // plugin and see the unchanged env_keys).
          if (orphanKeys.length > 0) {
            const proceed = confirm(
              t('mcp.custom.orphanEnvWarning', String(orphanKeys.length), orphanKeys.join(', ')),
            );
            if (proceed) {
              try {
                const cleanup = await mcpsApi.cleanupOrphanEnv(editingCustomServerId, orphanKeys);
                toast(
                  t('mcp.custom.orphanEnvCleaned',
                    String(cleanup.total_keys_removed),
                    String(cleanup.configs_updated)),
                  'success',
                );
              } catch (cleanupErr) {
                console.warn('cleanup_orphan_env failed:', cleanupErr);
                toast(t('mcp.custom.orphanEnvCleanFailed', userError(cleanupErr)), 'error');
              }
            }
          }
          resetAddMcp();
          refetchMcps();
        } catch (e) {
          console.warn('Failed to update Custom API:', e);
          toast(t('mcp.custom.error', userError(e)), 'error');
        }
        return;
      }
      try {
        const display = await mcpsApi.createConfig({
          server_id: 'api-custom',
          label: addMcpLabel || customName,
          env: {},
          args_override: null,
          is_global: addMcpGlobal,
          project_ids: addMcpProjectIds,
          host_sync: 'None',
          custom_spec: {
            name: customName.trim(),
            base_url: customBaseUrl.trim(),
            description: customDescription.trim(),
            docs_url: customDocsUrl.trim() || null,
            fields: customFields.filter(f => f.label.trim() !== ''),
            // 0.8.6 — drop blank-path rows the user added but never
            // filled (or the trailing "Add row" sentinel). Backend
            // does this too but client-side filter keeps the POST
            // payload lean and the "Empty endpoints?" hint on the
            // resulting plugin accurate.
            endpoints: customEndpoints.filter(e => e.path.trim() !== ''),
            auth: customAuth,
          },
        });
        resetAddMcp();
        pendingSelectConfigIdRef.current = display.id;
        refetchMcps();
        // KT-831 — the backend merges into an identical pre-existing
        // config instead of creating a duplicate (`merged_into_existing`);
        // say so instead of silently reporting "created" for a config
        // whose label/scope choice from THIS request was actually dropped.
        toast(
          display.merged_into_existing
            ? t('mcp.addMerged', display.label)
            : t('mcp.custom.created', customName.trim()),
          display.merged_into_existing ? 'warning' : 'success',
        );
      } catch (e) {
        console.warn('Failed to add Custom API:', e);
        toast(t('mcp.custom.error', userError(e)), 'error');
      }
      return;
    }
    // Registry path — the custom/edit branch above returned, so reaching
    // here means a registry tile is selected. Explicit guard narrows the
    // `string | null` for TS (the compound guard at the top can't).
    if (!addMcpSelected) return;
    try {
      const display = await mcpsApi.createConfig({
        server_id: addMcpSelected,
        label: addMcpLabel || mcpRegistry.find(m => m.id === addMcpSelected)?.name || 'New MCP',
        env: compactPluginCredentials(
          mcpRegistry.find(m => m.id === addMcpSelected),
          addMcpEnv,
        ),
        args_override: null,
        is_global: addMcpGlobal,
        project_ids: addMcpProjectIds,
        host_sync: addMcpHostSync ? 'GlobalOnly' : 'None',
      });
      resetAddMcp();
      pendingSelectConfigIdRef.current = display.id;
      refetchMcps();
      if (display.merged_into_existing) {
        toast(t('mcp.addMerged', display.label), 'warning');
      }
      // KT-831 — open the fiche of the config that now carries this
      // request (freshly created, or the pre-existing one it merged
      // into) instead of leaving the operator back at an empty grid.
    } catch (e) {
      console.warn('Failed to add MCP config:', e);
      toast(t('mcp.custom.error', userError(e)), 'error');
    }
  };

  // The plugin detail/edit is a non-blocking side panel. Esc is staged:
  // while editing, first Esc cancels the edit (back to the view body);
  // a second Esc (or X) closes the panel.
  // resetAddMcp only invokes stable setters, so the captured closure is safe.
  useEffect(() => {
    if (!selectedConfigId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (customForm.editingCustomServerId) { resetAddMcp(); return; }
      setSelectedConfigId(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // (0.8.11 — the old `eslint-disable react-hooks/exhaustive-deps` here was
    // dead: with the React-19 ruleset the rule no longer reports on this hook,
    // so the directive itself warned as unused. Removed rather than restored.)
  }, [selectedConfigId, setSelectedConfigId, customForm.editingCustomServerId, resetAddMcp]);

  const showBuiltinFallback = !list.builtinConfig && !addRegistry.showAddMcp && list.builtinMatchesList;

  return {
    t, isMobile, toast, ToastContainer,
    projects, mcpOverview, mcpRegistry, refetchMcps, installedAgentTypes, configLanguage,
    ...list,
    ...addRegistry,
    ...customForm,
    resetAddMcp, handleAddMcpModalKeyDown, handleAddMcpFromRegistry,
    showBuiltinFallback,
  };
}
