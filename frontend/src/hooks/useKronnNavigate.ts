import { useNavigate } from 'react-router';
import { PAGE_PATHS } from '../lib/routeConstants';
import type { Page } from '../lib/routeConstants';

export function useKronnNavigate() {
  const navigate = useNavigate();

  return {
    toPage: (page: Page) => navigate(PAGE_PATHS[page]),
    toProjects: () => navigate(PAGE_PATHS.projects),
    toProject: (id: string) => navigate(`/projects/${id}`),
    toDiscussions: (opts?: { focusBatchId?: string }) =>
      navigate(PAGE_PATHS.discussions, opts ? { state: opts } : undefined),
    toDiscussion: (id: string, opts?: { autoRun?: boolean; focusBatchId?: string }) =>
      navigate(`/discussions/${id}`, opts ? { state: opts } : undefined),
    toPlanning: () => navigate(PAGE_PATHS.planning),
    toPlanningTask: (taskId: string) => navigate(`/planning/${taskId}`),
    toPlugins: () => navigate(PAGE_PATHS.mcps),
    toPlugin: (configId: string) => navigate(`/plugins/${configId}`),
    toWorkflows: () => navigate(PAGE_PATHS.workflows),
    toWorkflow: (id: string) => navigate(`/workflows/${id}`),
    toWorkflowRun: (wfId: string, runId: string) => navigate(`/workflows/${wfId}/runs/${runId}`),
    toQuickPrompts: () => navigate('/workflows/qp'),
    toQuickPrompt: (id: string) => navigate(`/workflows/qp/${id}`),
    toQuickApis: () => navigate('/workflows/qa'),
    toQuickApi: (id: string) => navigate(`/workflows/qa/${id}`),
    toConfig: (section?: string) => navigate(`/config${section ? `#${section}` : ''}`),
    launchWorkflowPreset: (presetId: string, projectId: string) =>
      navigate(PAGE_PATHS.workflows, { state: { pendingPreset: { presetId, projectId } } }),
    batchLaunched: (firstDiscId: string, batchRunId: string) =>
      navigate(`/discussions/${firstDiscId}`, { state: { focusBatchId: batchRunId } }),
  };
}
