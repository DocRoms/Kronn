import { quickApis, quickExecs, quickPrompts, workflows } from './api';
import type { ProjectRepositoryResourceKind, Workflow } from '../types/generated';

export type ExecutionField = 'binary' | 'arguments' | 'folder' | 'triggers' | 'network' | 'steps' | 'agent' | 'prompt';

export interface ExecutionSummary {
  fields: Array<{ field: ExecutionField; values: string[] }>;
}

const PROMPT_EXCERPT = 400;

function triggerLabel(trigger: Workflow['trigger']): string {
  if (trigger.type === 'Cron') return `cron ${trigger.schedule}`;
  if (trigger.type === 'Tracker') {
    return `${trigger.source.owner}/${trigger.source.repo} · ${trigger.query} (${trigger.interval})`;
  }
  return 'manual';
}

function workflowSummary(workflow: Workflow): ExecutionSummary {
  const network = [
    ...workflow.steps
      .filter(step => ['ApiCall', 'BatchApiCall', 'CollectApiData'].includes(step.step_type.type))
      .map(step => `${step.name} (${step.step_type.type})`),
    ...workflow.actions.map(action => `${action.type}`),
  ];
  return {
    fields: [
      { field: 'triggers', values: [triggerLabel(workflow.trigger)] },
      {
        field: 'steps',
        values: workflow.steps.map(step => `${step.name} · ${step.step_type.type}${step.step_type.type === 'Agent' ? ` · ${step.agent}` : ''}`),
      },
      // Without isolation the run happens in the project's own checkout.
      { field: 'folder', values: [workflow.workspace_config?.require_isolation ? 'worktree' : 'project'] },
      { field: 'network', values: network },
    ],
  };
}

/** Refusing an imported definition removes Kronn's copy only; the repository
 *  file is left alone. */
export async function removeFromKronn(kind: ProjectRepositoryResourceKind, id: string): Promise<void> {
  if (kind === 'workflow') await workflows.delete(id);
  else if (kind === 'quick_prompt') await quickPrompts.delete(id);
  else if (kind === 'quick_api') await quickApis.delete(id);
  else if (kind === 'quick_exec') await quickExecs.delete(id);
}

/** What an imported definition would run, read from the Kronn copy (the
 *  repository listing carries no execution detail). `null` when the copy is
 *  gone or the kind runs nothing. */
export async function loadExecutionSummary(
  kind: ProjectRepositoryResourceKind,
  id: string,
): Promise<ExecutionSummary | null> {
  if (kind === 'workflow') return workflowSummary(await workflows.get(id));
  if (kind === 'quick_exec') {
    const item = (await quickExecs.list()).find(entry => entry.id === id);
    if (!item) return null;
    return {
      fields: [
        { field: 'binary', values: [item.command] },
        { field: 'arguments', values: item.args },
        { field: 'folder', values: ['project'] },
      ],
    };
  }
  if (kind === 'quick_api') {
    const item = (await quickApis.list()).find(entry => entry.id === id);
    if (!item) return null;
    return {
      fields: [
        { field: 'network', values: [`${item.api_method ?? 'GET'} ${item.api_plugin_slug}${item.api_endpoint_path}`] },
      ],
    };
  }
  if (kind === 'quick_prompt') {
    const item = (await quickPrompts.list()).find(entry => entry.id === id);
    if (!item) return null;
    return {
      fields: [
        { field: 'agent', values: [String(item.agent)] },
        { field: 'prompt', values: [item.prompt_template.slice(0, PROMPT_EXCERPT)] },
      ],
    };
  }
  return null;
}
