import type {
  PlanningPlacement,
  PlanningTaskPriority,
  PlanningTaskStatus,
} from '../types/generated';

export type PlanningProposal =
  | {
      action: 'create';
      title: string;
      description?: string;
      priority?: PlanningTaskPriority;
      placement?: PlanningPlacement;
      is_primary?: boolean;
    }
  | {
      action: 'create_many';
      tasks: Array<{
        title: string;
        description?: string;
        priority?: PlanningTaskPriority;
        placement?: PlanningPlacement;
      }>;
    }
  | {
      action: 'status';
      task_id: string;
      status: PlanningTaskStatus;
    }
  | { action: 'complete'; task_id: string }
  | { action: 'unblock'; task_id: string }
  | { action: 'open' };

const PRIORITIES: PlanningTaskPriority[] = ['critical', 'high', 'normal', 'low'];
const PLACEMENTS: PlanningPlacement[] = ['active', 'later'];
const STATUSES: PlanningTaskStatus[] = ['idea', 'todo', 'in_progress', 'blocked', 'done', 'archived'];

function optionalString(value: unknown): string | undefined | null {
  return value === undefined
    ? undefined
    : typeof value === 'string'
      ? value
      : null;
}

function parseCreateTask(value: unknown) {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Record<string, unknown>;
  if (typeof candidate.title !== 'string' || !candidate.title.trim()) return null;
  const description = optionalString(candidate.description);
  if (description === null) return null;
  const priority = candidate.priority === undefined
    ? undefined
    : PRIORITIES.find(item => item === candidate.priority);
  const placement = candidate.placement === undefined
    ? undefined
    : PLACEMENTS.find(item => item === candidate.placement);
  if (candidate.priority !== undefined && !priority) return null;
  if (candidate.placement !== undefined && !placement) return null;
  return {
    title: candidate.title,
    ...(description === undefined ? {} : { description }),
    ...(priority ? { priority } : {}),
    ...(placement ? { placement } : {}),
  };
}

export function parsePlanningProposal(value: unknown): PlanningProposal | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Record<string, unknown>;
  if (candidate.action === 'open') return { action: 'open' };
  if (candidate.action === 'create') {
    const task = parseCreateTask(candidate);
    if (!task) return null;
    if (candidate.is_primary !== undefined && typeof candidate.is_primary !== 'boolean') return null;
    return {
      action: 'create',
      ...task,
      ...(typeof candidate.is_primary === 'boolean' ? { is_primary: candidate.is_primary } : {}),
    };
  }
  if (candidate.action === 'create_many' && Array.isArray(candidate.tasks) && candidate.tasks.length > 0) {
    const tasks = candidate.tasks.map(parseCreateTask);
    if (tasks.some(task => task === null)) return null;
    return { action: 'create_many', tasks: tasks.filter(task => task !== null) };
  }
  if (typeof candidate.task_id !== 'string' || !candidate.task_id.trim()) return null;
  if (candidate.action === 'complete') {
    return { action: 'complete', task_id: candidate.task_id };
  }
  if (candidate.action === 'unblock') {
    return { action: 'unblock', task_id: candidate.task_id };
  }
  if (candidate.action === 'status') {
    const status = STATUSES.find(item => item === candidate.status);
    return status ? { action: 'status', task_id: candidate.task_id, status } : null;
  }
  return null;
}
