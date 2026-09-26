import { useState, useRef, useEffect } from 'react';
import type { McpOverview, McpDefinition } from '../../types/generated';
import { pluginKind, type PluginKind } from '../../lib/pluginKind';

interface UseAddPluginRegistryStateArgs {
  mcpOverview: McpOverview;
  mcpRegistry: McpDefinition[];
}

/** Add-MCP modal's registry-browsing state: the type/category filters,
 *  the selected registry tile, and the per-plugin env form for a
 *  registry pick (as opposed to the Custom API form, which owns its
 *  own state in `useCustomApiFormState`). Split out of the monolithic
 *  `useMcpPageState` (KT-830) to stay under the page's per-file line
 *  budget. */
export function useAddPluginRegistryState({ mcpOverview, mcpRegistry }: UseAddPluginRegistryStateArgs) {
  const [showAddMcp, setShowAddMcp] = useState(false);
  const [addMcpSearch, setAddMcpSearch] = useState('');
  // 0.8.6 phase 4 — top-level type filter (audit feedback 2026-05-22).
  // Lets the user narrow the discovery dropdown to a specific TRANSPORT
  // kind (MCP / API / CLI). Defaults to 'all' so the existing behaviour
  // (every plugin visible) is preserved when nothing is selected. The
  // category-by-tag grouping below continues to apply within the
  // filtered subset.
  const [addMcpKindFilter, setAddMcpKindFilter] = useState<'all' | PluginKind>('all');
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [addMcpSelected, setAddMcpSelected] = useState<string | null>(null);
  const [addMcpLabel, setAddMcpLabel] = useState('');
  const [addMcpEnv, setAddMcpEnv] = useState<Record<string, string>>({});
  const [addMcpGlobal, setAddMcpGlobal] = useState(false);
  const [addMcpHostSync, setAddMcpHostSync] = useState(false);
  const [addVisibleFields, setAddVisibleFields] = useState<Set<string>>(new Set());
  const addMcpRef = useRef<HTMLDivElement>(null);
  const addMcpTriggerRef = useRef<HTMLButtonElement>(null);
  const addMcpWasOpenRef = useRef(false);

  useEffect(() => {
    if (showAddMcp) {
      addMcpWasOpenRef.current = true;
      return;
    }
    if (addMcpWasOpenRef.current) {
      addMcpWasOpenRef.current = false;
      addMcpTriggerRef.current?.focus();
    }
  }, [showAddMcp]);

  const { configs } = mcpOverview;
  const configuredServerIds = new Set(configs.map(c => c.server_id));
  const availableRegistry = mcpRegistry.filter(m =>
    // Pinned separately at the top of the grid — keep it out of the
    // categorized list to avoid duplicating it under "Other".
    m.id !== 'api-custom' &&
    (!addMcpSearch || m.name.toLowerCase().includes(addMcpSearch.toLowerCase()) || m.tags.some(tag => tag.toLowerCase().includes(addMcpSearch.toLowerCase()))) &&
    // 0.8.6 phase 4 — type filter. `'all'` lets every plugin through,
    // otherwise we narrow to exactly that PluginKind. `'mcp'` is the
    // permissive default that ALSO matches `hybrid` (a hybrid plugin
    // is still primarily an MCP transport from the user's standpoint).
    (
      addMcpKindFilter === 'all'
        ? true
        : addMcpKindFilter === 'mcp'
          ? (pluginKind(m) === 'mcp' || pluginKind(m) === 'hybrid')
          : pluginKind(m) === addMcpKindFilter
    )
  );
  const selectedDef = mcpRegistry.find(m => m.id === addMcpSelected);
  // Whether the pinned Custom API tile should be visible. Hide it when the
  // user searches for something that doesn't match "custom" / "api" so the
  // tile doesn't fight for attention when they're clearly looking for
  // GitHub etc.
  const customApiVisible = !addMcpSearch
    || 'custom api'.includes(addMcpSearch.toLowerCase())
    || addMcpSearch.toLowerCase().includes('custom')
    || addMcpSearch.toLowerCase().includes('api');

  return {
    showAddMcp, setShowAddMcp, addMcpSearch, setAddMcpSearch,
    addMcpKindFilter, setAddMcpKindFilter, selectedCategory, setSelectedCategory,
    addMcpSelected, setAddMcpSelected, addMcpLabel, setAddMcpLabel,
    addMcpEnv, setAddMcpEnv, addMcpGlobal, setAddMcpGlobal,
    addMcpHostSync, setAddMcpHostSync, addVisibleFields, setAddVisibleFields,
    addMcpRef, addMcpTriggerRef,

    configuredServerIds, availableRegistry, selectedDef, customApiVisible,
  };
}
