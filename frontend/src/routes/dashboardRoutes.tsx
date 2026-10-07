import type { ReactElement } from 'react';
import type { RouteObject } from 'react-router';
import { EMBED_SETTINGS_SEGMENT, PAGE_PATHS, type DashboardPage } from '../lib/routes';
import { HomeRedirect } from './HomeRedirect';
import { RouteShell } from './RouteShell';
import {
  DiscussionsRoute, PagesRoute, PlanningRoute, PluginsRoute, ProjectsRoute, SettingsRoute, WorkflowsRoute,
} from './lazyRoutes';

// The shell is keyed by page: every route renders at the same outlet position,
// so without a key React would carry one page's error zone over to the next.
function pageRoute(page: DashboardPage, label: string, element: ReactElement, params = ''): RouteObject {
  return {
    path: `${PAGE_PATHS[page]}${params}`,
    element: <RouteShell key={page} label={label}>{element}</RouteShell>,
  };
}

/** The pages rendered in the dashboard outlet. A pure route table. */
export const dashboardRoutes: RouteObject[] = [
  { index: true, element: <HomeRedirect /> },
  pageRoute('projects', 'Projects', <ProjectsRoute />, '/:projectId?'),
  pageRoute('discussions', 'Discussions', <DiscussionsRoute />, '/:discussionId?'),
  pageRoute('planning', 'Planning', <PlanningRoute />, '/:taskId?'),
  // One route per shape of the Automation address; all render the same page.
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/:workflowId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/:workflowId/runs/:runId'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qp/:qpId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qa/:qaId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qe/:qeId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/skills/:skillId?'),
  pageRoute('pages', 'Artifacts', <PagesRoute />, '/:pageId?'),
  pageRoute('mcps', 'Plugins', <PluginsRoute />, '/:configId?'),
  pageRoute('settings', 'Settings', <SettingsRoute />),
  pageRoute('settings', 'Settings', <SettingsRoute />, EMBED_SETTINGS_SEGMENT),
  { path: '*', element: <HomeRedirect /> },
];
