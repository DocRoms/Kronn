import type { ReactElement } from 'react';
import type { RouteObject } from 'react-router';
import {
  DESIGN_SYSTEM_ROUTE, DISCUSSION_COMPARE_ROUTE, EMBED_SETTINGS_SEGMENT, PAGE_PATHS, WORKFLOW_EDITOR_ROUTES, type DashboardPage,
} from '../lib/routes';
import { HomeRedirect } from './HomeRedirect';
import { RouteShell } from './RouteShell';
import {
  DesignSystemRoute, DiscussionsRoute, PagesRoute, PlanningRoute, PluginsRoute, ProjectsRoute, SettingsRoute, WorkflowsRoute,
} from './lazyRoutes';

// The shell is keyed by page: every route renders at the same outlet position,
// so without a key React would carry one page's error zone over to the next.
// A sub-page that is a page of its own (the design system under Configuration)
// takes its own key.
function pageRoute(page: DashboardPage, label: string, element: ReactElement, params = '', shellKey: string = page): RouteObject {
  return {
    path: `${PAGE_PATHS[page]}${params}`,
    element: <RouteShell key={shellKey} label={label}>{element}</RouteShell>,
  };
}

/** The pages rendered in the dashboard outlet. A pure route table. */
export const dashboardRoutes: RouteObject[] = [
  { index: true, element: <HomeRedirect /> },
  pageRoute('projects', 'Projects', <ProjectsRoute />, '/:projectId?'),
  pageRoute('projects', 'Projects', <ProjectsRoute />, '/:projectId/:projectView'),
  pageRoute('discussions', 'Discussions', <DiscussionsRoute />, '/:discussionId?'),
  pageRoute('discussions', 'Discussions', <DiscussionsRoute />, DISCUSSION_COMPARE_ROUTE),
  pageRoute('planning', 'Planning', <PlanningRoute />, '/:taskId?'),
  // One route per shape of the Automation address; all render the same page.
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/:workflowId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/:workflowId/runs/:runId'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, WORKFLOW_EDITOR_ROUTES.create),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, WORKFLOW_EDITOR_ROUTES.edit),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qp/:qpId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qa/:qaId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/qe/:qeId?'),
  pageRoute('workflows', 'Workflows', <WorkflowsRoute />, '/skills/:skillId?'),
  pageRoute('pages', 'Artifacts', <PagesRoute />, '/:pageId?'),
  pageRoute('mcps', 'Plugins', <PluginsRoute />, '/:configId?'),
  pageRoute('settings', 'Settings', <SettingsRoute />),
  pageRoute('settings', 'Settings', <SettingsRoute />, EMBED_SETTINGS_SEGMENT),
  pageRoute('settings', 'Design system', <DesignSystemRoute />, DESIGN_SYSTEM_ROUTE, 'settings-design-system'),
  { path: '*', element: <HomeRedirect /> },
];
