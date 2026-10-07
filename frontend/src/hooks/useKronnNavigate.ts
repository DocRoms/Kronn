import { useLayoutEffect, useMemo, useRef } from 'react';
import { useNavigate } from 'react-router';
import {
  PAGE_PATHS, automationPath, discussionPath, livePagePath, planningTaskPath, pluginPath, projectPath, workflowPath,
  type AutomationIntent, type AutomationSelection, type DashboardPage, type DiscussionsIntent,
} from '../lib/routes';

export interface KronnNavigateOptions {
  /** Replace the current history entry instead of adding one (redirects). */
  replace?: boolean;
}

export interface KronnNavigate {
  toPage: (page: DashboardPage, options?: KronnNavigateOptions) => void;
  /** The Projects page with this project open. */
  toProject: (projectId: string, options?: KronnNavigateOptions) => void;
  /** The Discussions page with this discussion open, and what to do on arrival. */
  toDiscussion: (discussionId: string, options?: KronnNavigateOptions & DiscussionsIntent) => void;
  /** The Discussions page as it was left, and what to do on arrival. */
  toDiscussions: (intent?: DiscussionsIntent) => void;
  /** The Planning page with this task open. */
  toPlanningTask: (taskId: string, options?: KronnNavigateOptions) => void;
  /** The Automation page on this workflow, optionally revealing one of its runs. */
  toWorkflow: (workflowId: string, runId?: string | null, options?: KronnNavigateOptions) => void;
  /** The Automation page on this tab and resource. */
  toAutomation: (selection: AutomationSelection, options?: KronnNavigateOptions) => void;
  /** The Automation page as it was left, and what to do on arrival. */
  toWorkflows: (intent?: AutomationIntent) => void;
  /** The Plugins page with this config open. */
  toPlugin: (configId: string, options?: KronnNavigateOptions) => void;
  /** The Artifacts page with this Page open. */
  toLivePage: (pageId: string, options?: KronnNavigateOptions) => void;
}

function intentState({ autoRun, focusBatch }: DiscussionsIntent): DiscussionsIntent | null {
  const intent: DiscussionsIntent = {};
  if (autoRun) intent.autoRun = true;
  if (focusBatch) intent.focusBatch = focusBatch;
  return Object.keys(intent).length > 0 ? intent : null;
}

/**
 * Typed navigation between Kronn addresses. Components go through this hook
 * rather than `useNavigate`, so no path is ever built outside `lib/routes`.
 *
 * The returned object is stable for the component's whole life: it is safe
 * in effect and callback dependency lists. The router's own `navigate` is
 * rebuilt when the matched route changes (the Automation page spans several
 * routes), so it is read through a ref; every path here is absolute, so
 * nothing depends on the route it is called from.
 */
export function useKronnNavigate(): KronnNavigate {
  const latest = useNavigate();
  const navigateRef = useRef(latest);
  useLayoutEffect(() => { navigateRef.current = latest; }, [latest]);
  return useMemo(() => ({
    toPage: (page, options) => {
      void navigateRef.current(PAGE_PATHS[page], { replace: options?.replace });
    },
    toProject: (projectId, options) => {
      void navigateRef.current(projectPath(projectId), { replace: options?.replace });
    },
    toDiscussion: (discussionId, options = {}) => {
      void navigateRef.current(discussionPath(discussionId), { replace: options.replace, state: intentState(options) });
    },
    toDiscussions: (intent = {}) => {
      void navigateRef.current(PAGE_PATHS.discussions, { state: intentState(intent) });
    },
    toPlanningTask: (taskId, options) => {
      void navigateRef.current(planningTaskPath(taskId), { replace: options?.replace });
    },
    toWorkflow: (workflowId, runId, options) => {
      void navigateRef.current(workflowPath(workflowId, runId), { replace: options?.replace });
    },
    toAutomation: (selection, options) => {
      void navigateRef.current(automationPath(selection), { replace: options?.replace });
    },
    toWorkflows: (intent = {}) => {
      void navigateRef.current(PAGE_PATHS.workflows, { state: intent.preset ? { preset: intent.preset } : null });
    },
    toPlugin: (configId, options) => {
      void navigateRef.current(pluginPath(configId), { replace: options?.replace });
    },
    toLivePage: (pageId, options) => {
      void navigateRef.current(livePagePath(pageId), { replace: options?.replace });
    },
  }), []);
}
