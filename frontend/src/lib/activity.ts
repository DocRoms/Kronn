// The attention center's client side (KT-1658, phase 1 of KT-334).
//
// The panel renders the generic `ActivityRow` facts and nothing about what a
// row is: its compartment decides where it sits, its `type` only names it, and
// what it can do is a list of actions the backend offers. This module holds the
// few lookups that stay outside the panel — compartments, action handlers,
// deep links — so a new kind of row needs an entry here at most, never a panel
// change.

import {
  discussions as discussionsApi,
  orchestration as orchestrationApi,
  projects as projectsApi,
  workflows as workflowsApi,
} from './api';
import { discussionPath, planningTaskPath, projectPath, workflowPath } from './routes';
import type { KronnNavigate } from '../hooks/useKronnNavigate';
import type {
  ActivityAction,
  ActivityCompartment,
  ActivityLink,
  ActivityOverview,
  ActivityRow,
  ActivityState,
} from '../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/**
 * The compartments of the panel, in reading order (KT-334). Phase 1 fills
 * `running`; the other two are reserved here so a later phase only starts
 * emitting rows into them. A reserved compartment shows once it has a row.
 */
export const ACTIVITY_COMPARTMENTS: readonly {
  id: ActivityCompartment;
  /** Shown with an empty-state even when it has no row. */
  alwaysShown: boolean;
}[] = [
  { id: 'needs_decision', alwaysShown: false },
  { id: 'running', alwaysShown: true },
  { id: 'unseen_done', alwaysShown: false },
];

const STATE_ORDER: Record<ActivityState, number> = { running: 0, queued: 1, paused: 2 };

/** The rows of one compartment: working before queued before paused, oldest first. */
export function rowsOfCompartment(
  rows: readonly ActivityRow[],
  compartment: ActivityCompartment,
): ActivityRow[] {
  return rows
    .filter(row => row.compartment === compartment)
    .sort((a, b) =>
      (STATE_ORDER[a.state] ?? 3) - (STATE_ORDER[b.state] ?? 3)
      || new Date(a.started_at).getTime() - new Date(b.started_at).getTime());
}

/** Something is in progress: the bell moves. A paused row is waiting, not working. */
export function isAnythingRunning(overview: ActivityOverview | null | undefined): boolean {
  return (overview?.rows ?? []).some(row => row.compartment === 'running' && row.state !== 'paused');
}

/** What waits for the owner: the bell's counter. Empty until the decision compartment exists. */
export function waitingOnOwnerCount(overview: ActivityOverview | null | undefined): number {
  return (overview?.rows ?? []).filter(row => row.compartment === 'needs_decision').length;
}

/** The poll pace: quick while something moves, relaxed when idle. */
export const ACTIVITY_POLL_BUSY_MS = 3_000;
export const ACTIVITY_POLL_IDLE_MS = 15_000;

/** `4m 12s`, `1h 03m`: a duration read at a glance. */
export function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, '0')}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`;
  return `${Math.floor(hours / 24)}d ${String(hours % 24).padStart(2, '0')}h`;
}

/** A translation, or `fallback` when the key has none (a row type the locale files do not name yet). */
export function translateOr(t: Translate, key: string, fallback: string): string {
  const text = t(key);
  return text === key ? fallback : text;
}

/** `agent_turn` → `Agent turn`: how an unnamed row type reads. */
export function humanizeType(type: string): string {
  const spaced = type.replace(/[_-]+/g, ' ').trim();
  return spaced ? spaced.charAt(0).toUpperCase() + spaced.slice(1) : type;
}

/** The address a row opens, from the route table. */
export function activityLinkPath(link: ActivityLink): string {
  switch (link.target) {
    case 'discussion': return discussionPath(link.id);
    case 'workflow': return workflowPath(link.id, link.run_id);
    case 'project': return projectPath(link.id);
    case 'planning_task': return planningTaskPath(link.id);
  }
}

/** Opens a row's place in the app. */
export function openActivityLink(nav: KronnNavigate, link: ActivityLink): void {
  switch (link.target) {
    case 'discussion': nav.toDiscussion(link.id); return;
    case 'workflow': nav.toWorkflow(link.id, link.run_id); return;
    case 'project': nav.toProject(link.id); return;
    case 'planning_task': nav.toPlanningTask(link.id); return;
  }
}

/** Performs an action the backend offered on a row, with that action's `args`. */
export type ActivityActionHandler = (args: Readonly<Record<string, string>>) => Promise<unknown>;

/**
 * The existing call behind each action id: the same one the page's own button
 * makes, so the panel adds no second way to stop or resume anything.
 */
export const ACTIVITY_ACTION_HANDLERS: Readonly<Record<string, ActivityActionHandler>> = {
  stop_dispatch: args => discussionsApi.stopDispatch(args.discussion_id, args.dispatch_id),
  cancel_workflow_run: args => workflowsApi.cancelRun(args.workflow_id, args.run_id),
  resume_workflow_run: args => workflowsApi.resumeRun(args.run_id),
  cancel_audit: args => projectsApi.cancelAudit(args.project_id),
  cancel_task_execution: args =>
    orchestrationApi.cancelExecution(args.execution_id, 'Cancelled from the activity panel'),
};

/** The label key of an action: it follows what the action does, never the row's type. */
export function actionLabelKey(action: ActivityAction): string {
  return `activity.action.${action.kind}`;
}

/** Whether two overviews show the same thing (the clock stamp aside): a poll that
 *  found nothing new must not re-render the panel. */
export function sameActivity(a: ActivityOverview | null, b: ActivityOverview | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return JSON.stringify({ ...a, generated_at: '' }) === JSON.stringify({ ...b, generated_at: '' });
}
