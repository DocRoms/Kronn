import { useDashboardContext } from '../lib/dashboardContext';
import { SettingsPage } from '../pages/SettingsPage';
import { ErrorBoundary } from '../components/ErrorBoundary';

export function SettingsRoute() {
  const ctx = useDashboardContext();
  return (
    <ErrorBoundary mode="zone" label="Settings">
      <SettingsPage
        agents={ctx.agents}
        agentAccess={ctx.agentAccess}
        configLanguage={ctx.configLanguage}
        projects={ctx.projects}
        refetchAgents={ctx.refetchAgents}
        refetchAgentAccess={ctx.refetchAgentAccess}
        refetchLanguage={ctx.refetchLanguage}
        refetchProjects={ctx.refetch}
        refetchDiscussions={ctx.refetchDiscussions}
        onReset={ctx.onReset}
        toast={ctx.toast}
        hasConfiguredApi={ctx.mcpOverview.configs.some(cfg =>
          ctx.mcpOverview.servers.some(s => s.id === cfg.server_id && s.api_spec != null)
        )}
      />
    </ErrorBoundary>
  );
}
