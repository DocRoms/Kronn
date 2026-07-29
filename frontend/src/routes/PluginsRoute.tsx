import { useParams } from 'react-router';
import { useDashboardContext } from '../lib/dashboardContext';
import { McpPage } from '../pages/McpPage';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { isUsable } from '../lib/constants';

export function PluginsRoute() {
  const { configId } = useParams<{ configId?: string }>();
  const ctx = useDashboardContext();
  return (
    <ErrorBoundary mode="zone" label="Plugins">
      <McpPage
        projects={ctx.projects}
        mcpOverview={ctx.mcpOverview}
        mcpRegistry={ctx.mcpRegistry}
        refetchMcps={ctx.refetchMcps}
        initialSelectedConfigId={configId}
        installedAgentTypes={ctx.agents.filter(isUsable).map(a => a.agent_type)}
        configLanguage={ctx.configLanguage ?? undefined}
      />
    </ErrorBoundary>
  );
}
