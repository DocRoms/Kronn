import { useParams, useLocation } from 'react-router';
import { useDashboardContext } from '../lib/dashboardContext';
import { WorkflowsPage } from '../pages/WorkflowsPage';
import { ErrorBoundary } from '../components/ErrorBoundary';
import { isUsable } from '../lib/constants';

export function WorkflowsRoute() {
  const { workflowId, runId, qpId, qaId } = useParams<{
    workflowId?: string; runId?: string; qpId?: string; qaId?: string;
  }>();
  const { pathname } = useLocation();
  const ctx = useDashboardContext();

  const initialTab = pathname.startsWith('/workflows/qa')
    ? 'quickApis' as const
    : pathname.startsWith('/workflows/qp')
      ? 'quickPrompts' as const
      : 'workflows' as const;

  return (
    <ErrorBoundary mode="zone" label="Workflows">
      <WorkflowsPage
        projects={ctx.projects}
        installedAgentTypes={ctx.agents.filter(isUsable).map(a => a.agent_type)}
        agentAccess={ctx.agentAccess ?? undefined}
        configLanguage={ctx.configLanguage ?? undefined}
        initialWorkflowId={workflowId}
        initialRunId={runId}
        initialTab={initialTab}
        initialQpId={qpId}
        initialQaId={qaId}
        toast={ctx.toast}
        onBatchSendingMark={ctx.onBatchSendingMark}
        refetchDiscussions={ctx.refetchDiscussions}
      />
    </ErrorBoundary>
  );
}
