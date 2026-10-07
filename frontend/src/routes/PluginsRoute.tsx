import { useCallback, useLayoutEffect, useRef } from 'react';
import { useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { isUsable } from '../lib/constants';
import { useDashboardContext } from '../lib/dashboardContext';
import { McpPage } from '../pages/McpPage';

export function PluginsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open config: `/plugins/<id>` is the selection, and
  // picking or closing one is a step Back can undo.
  const { configId } = useParams<{ configId?: string }>();
  const addressed = useRef(configId);
  useLayoutEffect(() => { addressed.current = configId; }, [configId]);
  const selectConfig = useCallback((id: string | null) => {
    if (id === (addressed.current ?? null)) return;
    if (id) nav.toPlugin(id);
    else nav.toPage('mcps');
  }, [nav]);
  return (
    <McpPage
      projects={ctx.projects}
      mcpOverview={ctx.mcpOverview}
      mcpRegistry={ctx.mcpRegistry}
      refetchMcps={ctx.refetchMcps}
      favoritesReady={ctx.mcpOverviewLoaded}
      selectedConfigId={configId ?? null}
      onSelectedConfigChange={selectConfig}
      installedAgentTypes={ctx.agents.filter(isUsable).map(a => a.agent_type)}
      configLanguage={ctx.configLanguage ?? undefined}
    />
  );
}
