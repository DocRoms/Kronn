import { useCallback, useLayoutEffect, useRef } from 'react';
import { useParams } from 'react-router';
import { useKronnNavigate } from '../hooks/useKronnNavigate';
import { isUsable } from '../lib/constants';
import { useDashboardContext } from '../lib/dashboardContext';
import type { SelectionReason } from '../lib/routes';
import { McpPage } from '../pages/McpPage';

export function PluginsRoute() {
  const ctx = useDashboardContext();
  const nav = useKronnNavigate();
  // The address owns the open config: `/plugins/<id>` is the selection, and
  // picking or closing one is a step Back can undo. A config the page lets go
  // because the loaded list does not know it replaces the address instead.
  const { configId } = useParams<{ configId?: string }>();
  const addressed = useRef(configId);
  useLayoutEffect(() => { addressed.current = configId; }, [configId]);
  const selectConfig = useCallback((id: string | null, reason: SelectionReason) => {
    if (id === (addressed.current ?? null)) return;
    if (id) nav.toPlugin(id);
    else nav.toPage('mcps', { replace: reason === 'restore' });
  }, [nav]);
  return (
    <McpPage
      projects={ctx.projects}
      mcpOverview={ctx.mcpOverview}
      mcpRegistry={ctx.mcpRegistry}
      refetchMcps={ctx.refetchMcps}
      overviewLoaded={ctx.mcpOverviewLoaded}
      selectedConfigId={configId ?? null}
      onSelectedConfigChange={selectConfig}
      installedAgentTypes={ctx.agents.filter(isUsable).map(a => a.agent_type)}
      configLanguage={ctx.configLanguage ?? undefined}
    />
  );
}
